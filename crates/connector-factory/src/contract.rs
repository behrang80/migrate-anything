//! Live contract tests for a generated connector. Read-only by default; write probes are never
//! issued by this suite (writes are exercised only by an approved migration), so write
//! capabilities remain `Documented`, not `Verified`.

use ma_connector_rest::RestConnector;
use ma_connector_sdk::{Connector, RecordLookup, RecordSource};
use ma_types::*;

fn conforms(v: &Value, f: &FieldDef) -> bool {
    match (v, f.field_type) {
        (Value::Null, _) => f.nullable,
        (_, FieldType::Unknown) => true,
        (Value::Int(_), FieldType::Float | FieldType::Decimal | FieldType::Int) => true,
        (Value::Decimal(_), FieldType::Float | FieldType::Decimal) => true,
        (Value::Float(_), FieldType::Float | FieldType::Decimal) => true,
        (Value::String(s), FieldType::Date | FieldType::DateTime) => !matches!(Value::parse_as(s, f.field_type), Value::Invalid { .. }),
        (Value::String(_), FieldType::String | FieldType::Bytes) => true,
        (Value::Bool(_), FieldType::Bool) => true,
        (Value::Array(_), FieldType::Array) => true,
        (Value::Object(_), FieldType::Object) => true,
        _ => false,
    }
}

/// Run contract tests and upgrade verified capabilities in `manifest`.
pub async fn run_contract_tests(conn: &RestConnector, manifest: &mut CapabilityManifest) -> Vec<ContractTestResult> {
    let mut out = vec![];
    let mut push = |name: String, passed: bool, detail: String| out.push(ContractTestResult { name, passed, detail });
    match conn.check_connection().await {
        Ok(()) => push("connection".into(), true, "authenticated read succeeded".into()),
        Err(e) => {
            push("connection".into(), false, e.message);
            return out;
        }
    }
    let spec = conn.spec().clone();
    let mut read_ok = true;
    for r in spec.resources.iter().filter(|r| r.list.is_some()) {
        let first = match conn.read_batch(&r.name, None, 5).await {
            Ok(b) => b,
            Err(e) => {
                read_ok = false;
                push(format!("{}: read", r.name), false, e.message);
                continue;
            }
        };
        push(format!("{}: read", r.name), true, format!("{} record(s) on first page", first.records.len()));
        manifest.set_object(&r.name, "read", Evidence::Verified, None);
        // schema conformance
        let mut problems = vec![];
        for rec in &first.records {
            for f in &r.fields {
                match rec.fields.get(&f.name) {
                    None if f.required => problems.push(format!("{}: required field `{}` missing", rec.id, f.name)),
                    Some(v) if !conforms(v, f) => {
                        problems.push(format!("{}: `{}` = {:?} does not match {:?}", rec.id, f.name, v, f.field_type))
                    }
                    _ => {}
                }
            }
            for k in rec.fields.keys() {
                if r.field_def(k).is_none() {
                    problems.push(format!("{}: undocumented field `{k}`", rec.id));
                }
            }
        }
        problems.truncate(10);
        push(
            format!("{}: schema conformance", r.name),
            problems.is_empty(),
            if problems.is_empty() { "sampled records match the documented schema".into() } else { problems.join("; ") },
        );
        // pagination advances without repeats
        if let Some(next) = &first.next_cursor {
            match conn.read_batch(&r.name, Some(next), 5).await {
                Ok(second) => {
                    let ids1: std::collections::BTreeSet<_> = first.records.iter().map(|r| r.id.clone()).collect();
                    let overlap = second.records.iter().filter(|r| ids1.contains(&r.id)).count();
                    let ok = overlap == 0 && !second.records.is_empty();
                    push(
                        format!("{}: pagination", r.name),
                        ok,
                        format!("second page {} record(s), {overlap} repeated", second.records.len()),
                    );
                    if ok {
                        manifest.set_object(&r.name, "pagination", Evidence::Verified, None);
                        manifest.set("pagination", Evidence::Verified, None);
                    }
                }
                Err(e) => push(format!("{}: pagination", r.name), false, e.message),
            }
        }
        // lookup by key
        if let (Some(lk), Some(rec)) = (&r.lookup, first.records.first()) {
            match conn.lookup_by_key(&r.name, &lk.key_field, &rec.id).await {
                Ok(Some(found)) if found.id == rec.id => {
                    push(format!("{}: lookup by key", r.name), true, format!("found `{}`", rec.id));
                    manifest.set_object(&r.name, "lookup", Evidence::Verified, None);
                    manifest.set("lookup_by_key", Evidence::Verified, None);
                }
                Ok(other) => {
                    push(format!("{}: lookup by key", r.name), false, format!("lookup of `{}` returned {:?}", rec.id, other.map(|o| o.id)))
                }
                Err(e) => push(format!("{}: lookup by key", r.name), false, e.message),
            }
        }
    }
    if read_ok {
        manifest.set("read", Evidence::Verified, None);
        manifest.set("authentication", Evidence::Verified, None);
    }
    push(
        "write probes".into(),
        true,
        "not issued by contract tests (writes happen only in approved migrations); write capability stays Documented".into(),
    );
    out
}

trait FieldLookup {
    fn field_def(&self, name: &str) -> Option<&FieldDef>;
}

impl FieldLookup for ResourceSpec {
    fn field_def(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}
