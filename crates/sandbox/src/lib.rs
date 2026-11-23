//! WebAssembly extension sandbox.
//!
//! Isolation properties (all enforced, all tested):
//! * **No host imports.** A module that declares any import is rejected before instantiation.
//!   Without imports a module has no way to reach files, network, clock, environment, processes
//!   or the credential store — the only data it sees is the JSON input we copy into its memory.
//! * **CPU:** fuel metering per call.
//! * **Wall clock:** epoch interruption with a per-call deadline.
//! * **Memory:** `StoreLimits` cap on linear memory, one instance, bounded tables.
//! * **Fresh instance per call:** no state survives between records.
//! * **Fail closed:** if the engine cannot be configured with these limits, loading fails.
//!
//! ABI (version 1): exports `memory`, `alloc(len: i32) -> i32`,
//! `transform(ptr: i32, len: i32) -> i64` returning `(out_ptr << 32) | out_len` of UTF-8 JSON.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ma_types::{MaError, MaResult, Value};
use wasmtime::{Config, Engine, ExternType, Instance, Module, Store, StoreLimits, StoreLimitsBuilder};

#[derive(Debug, Clone)]
pub struct SandboxLimits {
    pub fuel_per_call: u64,
    pub max_memory_bytes: usize,
    pub deadline: Duration,
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        SandboxLimits {
            fuel_per_call: 50_000_000,
            max_memory_bytes: 16 * 1024 * 1024,
            deadline: Duration::from_secs(2),
            max_input_bytes: 1024 * 1024,
            max_output_bytes: 1024 * 1024,
        }
    }
}

const EPOCH_TICK: Duration = Duration::from_millis(10);

pub struct Sandbox {
    engine: Engine,
    limits: SandboxLimits,
    modules: Mutex<BTreeMap<String, Module>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct State {
    limits: StoreLimits,
}

impl Sandbox {
    pub fn new(limits: SandboxLimits) -> MaResult<Self> {
        let mut cfg = Config::new();
        cfg.consume_fuel(true);
        cfg.epoch_interruption(true);
        let engine = Engine::new(&cfg).map_err(|e| MaError::policy(format!("sandbox unavailable (fail closed): {e}")))?;
        let stop = Arc::new(AtomicBool::new(false));
        let (e2, s2) = (engine.clone(), stop.clone());
        std::thread::Builder::new()
            .name("ma-sandbox-epoch".into())
            .spawn(move || {
                while !s2.load(Ordering::SeqCst) {
                    std::thread::sleep(EPOCH_TICK);
                    e2.increment_epoch();
                }
            })
            .map_err(|e| MaError::policy(format!("sandbox unavailable (fail closed): {e}")))?;
        Ok(Sandbox { engine, limits, modules: Mutex::new(BTreeMap::new()), stop })
    }

    /// Validate a module (binary or WAT text) against the extension ABI and isolation rules.
    pub fn validate(&self, bytes: &[u8]) -> MaResult<Module> {
        let module = Module::new(&self.engine, bytes).map_err(|e| MaError::validation(format!("invalid WebAssembly module: {e}")))?;
        let imports: Vec<String> = module.imports().map(|i| format!("{}::{}", i.module(), i.name())).collect();
        if !imports.is_empty() {
            return Err(MaError::policy(format!(
                "extension rejected: it imports host functions {imports:?}; extensions may not import anything"
            )));
        }
        let mut has_mem = false;
        let mut has_alloc = false;
        let mut has_transform = false;
        for e in module.exports() {
            match (e.name(), e.ty()) {
                ("memory", ExternType::Memory(m)) => {
                    has_mem = true;
                    let min_bytes = m.minimum().saturating_mul(65536);
                    if min_bytes as usize > self.limits.max_memory_bytes {
                        return Err(MaError::policy("extension rejected: initial memory exceeds the sandbox limit"));
                    }
                }
                ("alloc", ExternType::Func(_)) => has_alloc = true,
                ("transform", ExternType::Func(_)) => has_transform = true,
                _ => {}
            }
        }
        if !(has_mem && has_alloc && has_transform) {
            return Err(MaError::validation("extension must export `memory`, `alloc` and `transform`"));
        }
        Ok(module)
    }

    pub fn load(&self, name: &str, bytes: &[u8]) -> MaResult<()> {
        let m = self.validate(bytes)?;
        self.modules.lock().expect("lock").insert(name.to_string(), m);
        Ok(())
    }

    /// Run a module once on JSON input in a fresh, limited instance.
    pub fn run(&self, name: &str, input: &serde_json::Value) -> MaResult<serde_json::Value> {
        let module = self
            .modules
            .lock()
            .expect("lock")
            .get(name)
            .cloned()
            .ok_or_else(|| MaError::not_found(format!("extension `{name}` is not loaded")))?;
        let input = serde_json::to_vec(input)?;
        if input.len() > self.limits.max_input_bytes {
            return Err(MaError::validation("extension input too large"));
        }
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.limits.max_memory_bytes)
            .instances(1)
            .tables(4)
            .table_elements(10_000)
            .memories(1)
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(&self.engine, State { limits });
        store.limiter(|s| &mut s.limits);
        store.set_fuel(self.limits.fuel_per_call).map_err(|e| MaError::internal(e.to_string()))?;
        let ticks = (self.limits.deadline.as_millis() / EPOCH_TICK.as_millis()).max(1) as u64;
        store.set_epoch_deadline(ticks);
        let trap = |e: wasmtime::Error| MaError::policy(format!("extension `{name}` aborted: {e}"));
        let instance = Instance::new(&mut store, &module, &[]).map_err(trap)?;
        let memory = instance.get_memory(&mut store, "memory").ok_or_else(|| MaError::validation("no memory export"))?;
        let alloc = instance.get_typed_func::<i32, i32>(&mut store, "alloc").map_err(|e| MaError::validation(e.to_string()))?;
        let transform =
            instance.get_typed_func::<(i32, i32), i64>(&mut store, "transform").map_err(|e| MaError::validation(e.to_string()))?;
        let ptr = alloc.call(&mut store, input.len() as i32).map_err(trap)?;
        memory
            .write(&mut store, ptr as u32 as usize, &input)
            .map_err(|_| MaError::policy(format!("extension `{name}` returned an out-of-bounds buffer")))?;
        let packed = transform.call(&mut store, (ptr, input.len() as i32)).map_err(trap)?;
        let (out_ptr, out_len) = ((packed as u64 >> 32) as usize, (packed as u64 & 0xffff_ffff) as usize);
        if out_len > self.limits.max_output_bytes {
            return Err(MaError::policy("extension output too large"));
        }
        let data = memory.data(&store);
        let end = out_ptr.checked_add(out_len).ok_or_else(|| MaError::policy("extension output out of bounds"))?;
        let out = data.get(out_ptr..end).ok_or_else(|| MaError::policy("extension output out of bounds"))?;
        serde_json::from_slice(out).map_err(|e| MaError::validation(format!("extension output is not JSON: {e}")))
    }

    /// Self-test required before activation: the module must transform a probe value without
    /// trapping and return JSON.
    pub fn self_test(&self, name: &str) -> MaResult<serde_json::Value> {
        self.run(name, &serde_json::json!("probe"))
    }
}

impl ma_mapping::ExtensionRunner for Sandbox {
    fn call(&self, name: &str, input: &Value) -> MaResult<Value> {
        let out = self.run(name, &input.to_plain_json())?;
        Ok(Value::from_plain_json(&out, true))
    }
}

/// A reference extension (WAT): uppercases ASCII letters of a JSON string input.
pub const UPPERCASE_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (global $next (mut i32) (i32.const 1024))
  (func (export "alloc") (param $len i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $next))
    (global.set $next (i32.add (global.get $next) (local.get $len)))
    (local.get $p))
  (func (export "transform") (param $ptr i32) (param $len i32) (result i64)
    (local $i i32) (local $c i32)
    (block $done
      (loop $l
        (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
        (local.set $c (i32.load8_u (i32.add (local.get $ptr) (local.get $i))))
        (if (i32.and (i32.ge_u (local.get $c) (i32.const 97)) (i32.le_u (local.get $c) (i32.const 122)))
          (then (i32.store8 (i32.add (local.get $ptr) (local.get $i)) (i32.sub (local.get $c) (i32.const 32)))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $l)))
    (i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32)) (i64.extend_i32_u (local.get $len)))))
"#;

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = r#"(memory (export "memory") 1) (func (export "alloc") (param i32) (result i32) i32.const 1024)"#;

    #[test]
    fn runs_reference_extension() {
        let sb = Sandbox::new(SandboxLimits::default()).unwrap();
        sb.load("upper", UPPERCASE_WAT.as_bytes()).unwrap();
        assert_eq!(sb.run("upper", &serde_json::json!("hello")).unwrap(), serde_json::json!("HELLO"));
        assert!(sb.self_test("upper").is_ok());
    }

    #[test]
    fn rejects_any_import() {
        let sb = Sandbox::new(SandboxLimits::default()).unwrap();
        let wat = format!(
            r#"(module (import "host" "get_secret" (func $s (param i32) (result i32))) {HEADER}
                (func (export "transform") (param i32 i32) (result i64) i64.const 0))"#
        );
        let err = sb.load("steal", wat.as_bytes()).unwrap_err();
        assert_eq!(err.class, ma_types::ErrorClass::Policy);
        let wasi = format!(
            r#"(module (import "wasi_snapshot_preview1" "path_open" (func (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32))) {HEADER}
                (func (export "transform") (param i32 i32) (result i64) i64.const 0))"#
        );
        assert!(sb.load("fs", wasi.as_bytes()).is_err());
    }

    #[test]
    fn infinite_loop_is_stopped() {
        let sb = Sandbox::new(SandboxLimits { fuel_per_call: 5_000_000, ..Default::default() }).unwrap();
        let wat = format!(r#"(module {HEADER} (func (export "transform") (param i32 i32) (result i64) (loop $l (br $l)) i64.const 0))"#);
        sb.load("spin", wat.as_bytes()).unwrap();
        let err = sb.run("spin", &serde_json::json!(1)).unwrap_err();
        assert_eq!(err.class, ma_types::ErrorClass::Policy);
    }

    #[test]
    fn wall_clock_deadline_without_fuel_exhaustion() {
        let sb = Sandbox::new(SandboxLimits { fuel_per_call: u64::MAX / 2, deadline: Duration::from_millis(100), ..Default::default() })
            .unwrap();
        let wat = format!(r#"(module {HEADER} (func (export "transform") (param i32 i32) (result i64) (loop $l (br $l)) i64.const 0))"#);
        sb.load("spin", wat.as_bytes()).unwrap();
        let start = std::time::Instant::now();
        assert!(sb.run("spin", &serde_json::json!(1)).is_err());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn memory_growth_is_capped() {
        let sb = Sandbox::new(SandboxLimits { max_memory_bytes: 2 * 65536, ..Default::default() }).unwrap();
        let wat = format!(
            r#"(module {HEADER} (func (export "transform") (param i32 i32) (result i64)
                 (drop (memory.grow (i32.const 100))) i64.const 0))"#
        );
        sb.load("grow", wat.as_bytes()).unwrap();
        assert!(sb.run("grow", &serde_json::json!(1)).is_err());
        let big = r#"(module (memory (export "memory") 64) (func (export "alloc") (param i32) (result i32) i32.const 0)
                     (func (export "transform") (param i32 i32) (result i64) i64.const 0))"#;
        assert!(sb.load("big", big.as_bytes()).is_err());
    }

    #[test]
    fn out_of_bounds_output_is_rejected() {
        let sb = Sandbox::new(SandboxLimits::default()).unwrap();
        let wat = format!(r#"(module {HEADER} (func (export "transform") (param i32 i32) (result i64) i64.const 0x7fff000000000010))"#);
        sb.load("oob", wat.as_bytes()).unwrap();
        assert!(sb.run("oob", &serde_json::json!(1)).is_err());
    }
}
