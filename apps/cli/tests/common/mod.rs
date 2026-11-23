//! Shared helpers for the end-to-end demonstrations.
#![allow(dead_code, unsafe_code)] // unsafe: std::env::set_var in test setup only

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Once;

use ma_core::{Actor, App, OperatorConsent, PlanRequest, PlanSummary, SpawnMode};
use ma_types::*;

static INIT: Once = Once::new();

pub fn init_env() {
    INIT.call_once(|| unsafe {
        std::env::set_var("MA_INSECURE_TEST_APPROVAL", "1");
        std::env::set_var("MA_WORKER_EXE", env!("CARGO_BIN_EXE_migrate-anything"));
    });
}

pub struct TestEnv {
    pub dir: tempfile::TempDir,
    pub app: App,
}

impl TestEnv {
    pub async fn new() -> TestEnv {
        init_env();
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(&dir.path().join("home")).await.unwrap();
        TestEnv { dir, app }
    }

    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    pub fn path(&self, p: &str) -> PathBuf {
        let p = self.dir.path().join(p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    pub async fn files_conn(&self, name: &str, dir: &Path, format: &str, env: Environment) {
        self.app
            .add_connection(
                ConnectionConfig {
                    name: name.into(),
                    kind: "files".into(),
                    environment: env,
                    settings: serde_json::json!({"path": dir, "format": format}),
                    credential_ref: None,
                    allowed_hosts: vec![],
                    connector_ref: None,
                },
                Actor::Cli,
            )
            .await
            .unwrap();
    }

    pub async fn pg_conn(&self, name: &str, url: &str, env: Environment) {
        self.app
            .add_connection(
                ConnectionConfig {
                    name: name.into(),
                    kind: "postgres".into(),
                    environment: env,
                    settings: serde_json::json!({"url": url}),
                    credential_ref: None,
                    allowed_hosts: vec![],
                    connector_ref: None,
                },
                Actor::Cli,
            )
            .await
            .unwrap();
    }

    pub async fn plan(&self, source: &str, dest: &str, mapping: MappingSpec, batch: usize) -> PlanSummary {
        let options = ExecutionOptions { batch_size: batch, ..Default::default() };
        self.app
            .create_plan(
                &PlanRequest {
                    source: source.into(),
                    destination: dest.into(),
                    mapping: Some(mapping),
                    pairs: vec![],
                    options: Some(options),
                },
                Actor::Cli,
            )
            .await
            .unwrap()
    }

    pub async fn approve(&self, plan_id: &str) {
        let consent = OperatorConsent::for_tests("test-operator").unwrap();
        self.app.approve_plan(plan_id, &consent, std::time::Duration::from_secs(3600), false).await.unwrap();
    }

    /// Approve, execute in-process, verify, and return the report.
    pub async fn execute(&self, plan_id: &str) -> (String, MigrationReport, VerificationResult) {
        self.approve(plan_id).await;
        let job = self.app.start_job(plan_id, SpawnMode::Inline, Actor::Cli).await.unwrap();
        self.app.run_worker(&job.job_id).await.unwrap();
        let v = self.app.verify_job(&job.job_id, Actor::Cli).await.unwrap();
        let r = self.app.report(&job.job_id).await.unwrap();
        println!("{}", r.to_text());
        for c in &v.checks {
            println!("{} {} {:?} {}", if c.passed { "pass" } else { "FAIL" }, c.name, c.object, c.detail);
        }
        (job.job_id, r, v)
    }
}

// ------------------------------------------------------------------ mapping helpers

pub fn f(target: &str, src: &str) -> FieldMapping {
    FieldMapping::accepted(target, Expr::field(src))
}

pub fn fx(target: &str, expr: Expr) -> FieldMapping {
    FieldMapping::accepted(target, expr)
}

pub fn cast(src: &str, to: FieldType) -> Expr {
    Expr::Cast { input: Box::new(Expr::field(src)), to }
}

pub fn reff(target: &str, src: &str, object: &str) -> FieldMapping {
    FieldMapping::accepted(target, Expr::Ref { input: Box::new(Expr::field(src)), object: object.into() })
}

pub fn enum_map(src: &str, pairs: &[(&str, &str)]) -> Expr {
    Expr::EnumMap {
        input: Box::new(Expr::field(src)),
        map: pairs.iter().map(|(a, b)| (a.to_string(), Value::String(b.to_string()))).collect::<BTreeMap<_, _>>(),
        on_unmapped: Unmapped::Fail,
    }
}

pub fn object(src: &str, dst: &str, key: Option<&str>, fields: Vec<FieldMapping>) -> ObjectMapping {
    ObjectMapping {
        source_object: src.into(),
        target_object: dst.into(),
        target_key_field: key.map(String::from),
        fields,
        unmapped_fields: UnmappedFieldPolicy::Archive,
        skip_fields: vec![],
        write_mode: if key.is_some() { WriteMode::Upsert } else { WriteMode::Insert },
    }
}

// ------------------------------------------------------------------ postgres helpers

pub fn pg_admin_url() -> Option<String> {
    match std::env::var("MA_TEST_PG_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            eprintln!("skipped: MA_TEST_PG_URL not set (PostgreSQL demonstrations need a server)");
            None
        }
    }
}

pub struct TempDb {
    admin: String,
    pub name: String,
    pub url: String,
}

impl TempDb {
    pub async fn create(admin: &str, prefix: &str) -> TempDb {
        use sqlx::Connection;
        let name = format!("{prefix}_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let mut c = sqlx::PgConnection::connect(admin).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\""))).execute(&mut c).await.unwrap();
        let base = admin.rsplit_once('/').map(|(a, _)| a).unwrap_or(admin);
        TempDb { admin: admin.into(), url: format!("{base}/{name}"), name }
    }

    pub async fn exec(&self, sql: &str) {
        use sqlx::Connection;
        let mut c = sqlx::PgConnection::connect(&self.url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string())).execute(&mut c).await.unwrap();
    }

    pub async fn scalar_i64(&self, sql: &str) -> i64 {
        use sqlx::Connection;
        let mut c = sqlx::PgConnection::connect(&self.url).await.unwrap();
        sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.to_string())).fetch_one(&mut c).await.unwrap()
    }

    pub async fn drop_db(self) {
        use sqlx::Connection;
        let mut c = sqlx::PgConnection::connect(&self.admin).await.unwrap();
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)", self.name))).execute(&mut c).await;
    }
}

/// Tiny deterministic PRNG for synthetic data.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn pick<'a>(&mut self, xs: &'a [&'a str]) -> &'a str {
        xs[self.below(xs.len() as u64) as usize]
    }
}
