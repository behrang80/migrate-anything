//! Demo 5 — Interrupted migration. Real worker processes are killed mid-run (by fault injection
//! in the worst window — after a destination write, before its checkpoint — and by an external
//! kill), restarted, and the result is verified: no duplicates, no gaps.

mod common;

use std::fmt::Write as _;
use std::process::Command;
use std::time::{Duration, Instant};

use common::*;
use ma_core::{Actor, OperatorConsent, SpawnMode};
use ma_types::*;

const ROWS: usize = 20_000;

fn write_source(dir: &std::path::Path) {
    let mut s = String::from("id,name,email,balance,joined\n");
    for i in 1..=ROWS {
        let _ = writeln!(s, "{i},Name {i},user{i}@example.com,{}.{:02},2020-01-{:02}", i * 3, i % 100, 1 + i % 28);
    }
    std::fs::write(dir.join("accounts.csv"), s).unwrap();
}

fn mapping(key: &str) -> MappingSpec {
    MappingSpec {
        mapping_version: 1,
        objects: vec![object(
            "accounts",
            "accounts",
            Some(key),
            vec![
                fx(key, cast("id", FieldType::String)),
                f("name", "name"),
                f("email", "email"),
                f("balance", "balance"),
                f("joined", "joined"),
            ],
        )],
    }
}

fn worker(env: &TestEnv, job: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_migrate-anything"));
    c.arg("--home").arg(env.home()).args(["worker", "run", "--job", job]).env("MA_LEASE_SECS", "1");
    c
}

async fn prepared_job(env: &TestEnv, dest: &str, key: &str) -> String {
    let plan = env.plan("src", dest, mapping(key), 500).await;
    assert!(plan.approvable, "{:?}", plan.blocking_risks);
    env.approve(&plan.plan_id).await;
    env.app.start_job(&plan.plan_id, SpawnMode::Inline, Actor::Cli).await.unwrap().job_id
}

async fn resume_and_verify(env: &TestEnv, job: &str) -> MigrationReport {
    tokio::time::sleep(Duration::from_millis(1500)).await; // crashed worker's lease expires
    env.app.resume_job(job, SpawnMode::Inline, Actor::Cli).await.unwrap();
    env.app.run_worker(job).await.unwrap();
    let v = env.app.verify_job(job, Actor::Cli).await.unwrap();
    let r = env.app.report(job).await.unwrap();
    println!("{}", r.to_text());
    assert!(v.discrepancies.is_empty(), "{:?}", v.discrepancies);
    r
}

fn jsonl_keys(path: &std::path::Path, key: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()[key].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn demo5a_crash_after_write_before_checkpoint_files() {
    let env = TestEnv::new().await;
    let src = env.path("src");
    write_source(&src);
    let out = env.path("out");
    env.app
        .add_connection(
            ConnectionConfig {
                name: "src".into(),
                kind: "files".into(),
                environment: Environment::Production,
                settings: serde_json::json!({"path": src, "format": "csv"}),
                credential_ref: None,
                allowed_hosts: vec![],
                connector_ref: None,
            },
            Actor::Cli,
        )
        .await
        .unwrap();
    env.files_conn("dst", &out, "jsonl", Environment::Production).await;
    let job = prepared_job(&env, "dst", "account_ref").await;

    // the worker writes batch 7 to the destination and dies before committing its checkpoint
    let status = worker(&env, &job).env("MA_ENABLE_FAULT_INJECTION", "1").env("MA_FAULT", "after_write:7").status().unwrap();
    assert_eq!(status.code(), Some(86), "worker was killed by fault injection");
    let partial = jsonl_keys(&out.join("accounts.jsonl"), "account_ref");
    // 6 batches are checkpointed; batch 7 may or may not have reached the file (it was in the
    // writer's buffer when the process died). Either way resume must truncate/redo it.
    assert!((6 * 500..=7 * 500).contains(&partial.len()), "unexpected partial size {}", partial.len());
    let info = env.app.job_status(&job).await.unwrap();
    assert_eq!(info.status, JobStatus::Running, "crashed job still looks running until a new worker takes over");
    assert_eq!(info.stages[0].counters.written(), 6 * 500);

    let report = resume_and_verify(&env, &job).await;
    let keys = jsonl_keys(&out.join("accounts.jsonl"), "account_ref");
    let unique: std::collections::BTreeSet<_> = keys.iter().collect();
    assert_eq!(keys.len(), ROWS, "no duplicates after resume");
    assert_eq!(unique.len(), ROWS, "no gaps after resume");
    assert_eq!(report.records_written, ROWS as u64);
    assert_eq!(report.final_status, FinalStatus::CompletedAndVerified);
    assert_eq!(env.app.job_status(&job).await.unwrap().attempts, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn demo5b_external_kill_of_detached_worker() {
    let env = TestEnv::new().await;
    let src = env.path("src");
    write_source(&src);
    let out = env.path("out");
    env.files_conn("src", &src, "csv", Environment::Production).await;
    env.files_conn("dst", &out, "csv", Environment::Production).await;
    let job = prepared_job(&env, "dst", "account_ref").await;

    // slow the worker down a little so it can be killed mid-run
    let mut child = worker(&env, &job).env("MA_ENABLE_FAULT_INJECTION", "0").spawn().unwrap();
    let start = Instant::now();
    loop {
        let j = env.app.job_status(&job).await.unwrap();
        let done = j.stages[0].counters.batches;
        if done >= 3 || j.status.is_terminal() || start.elapsed() > Duration::from_secs(60) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let _ = child.kill(); // hard kill (TerminateProcess / SIGKILL)
    let _ = child.wait();
    let j = env.app.job_status(&job).await.unwrap();
    println!("killed after {} committed batches, status {}", j.stages[0].counters.batches, j.status.as_str());

    let report = resume_and_verify(&env, &job).await;
    assert_eq!(report.records_written, ROWS as u64);
    assert_eq!(report.final_status, FinalStatus::CompletedAndVerified);
    let lines = std::fs::read_to_string(out.join("accounts.csv")).unwrap().lines().count();
    assert_eq!(lines, ROWS + 1, "header + one row per record");
}

#[tokio::test(flavor = "multi_thread")]
async fn demo5c_pause_resume_and_cancel() {
    let env = TestEnv::new().await;
    let src = env.path("src");
    write_source(&src);
    env.files_conn("src", &src, "csv", Environment::Production).await;
    env.files_conn("dst", &env.path("out"), "jsonl", Environment::Production).await;
    let job = prepared_job(&env, "dst", "account_ref").await;
    env.app.pause_job(&job, Actor::Cli).await.unwrap();
    env.app.run_worker(&job).await.unwrap();
    assert_eq!(env.app.job_status(&job).await.unwrap().status, JobStatus::Paused);
    env.app.resume_job(&job, SpawnMode::Inline, Actor::Cli).await.unwrap();
    env.app.run_worker(&job).await.unwrap();
    assert_eq!(env.app.job_status(&job).await.unwrap().status, JobStatus::Completed);

    // regression: a CSV destination (header already written) paused before its first checkpoint
    // must be resumable instead of failing with "destination file already exists"
    let out_csv = env.path("out_csv");
    env.files_conn("dstcsv", &out_csv, "csv", Environment::Production).await;
    let job_csv = prepared_job(&env, "dstcsv", "account_ref").await;
    env.app.pause_job(&job_csv, Actor::Cli).await.unwrap();
    env.app.run_worker(&job_csv).await.unwrap();
    assert_eq!(env.app.job_status(&job_csv).await.unwrap().status, JobStatus::Paused);
    env.app.resume_job(&job_csv, SpawnMode::Inline, Actor::Cli).await.unwrap();
    env.app.run_worker(&job_csv).await.unwrap();
    assert_eq!(env.app.job_status(&job_csv).await.unwrap().status, JobStatus::Completed);
    assert_eq!(std::fs::read_to_string(out_csv.join("accounts.csv")).unwrap().lines().count(), ROWS + 1);

    // cancel: a job cancelled before any worker runs ends as Cancelled and nothing is claimed
    env.files_conn("dst2", &env.path("out2"), "jsonl", Environment::Production).await;
    let job2 = prepared_job(&env, "dst2", "account_ref").await;
    env.app.cancel_job(&job2, Actor::Cli).await.unwrap();
    assert!(env.app.run_worker(&job2).await.is_err(), "a cancelled job cannot be run");
    let r = env.app.report(&job2).await.unwrap();
    assert_eq!(r.final_status, FinalStatus::Cancelled);
    assert_eq!(r.records_written, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn demo5d_crash_with_postgres_destination_is_idempotent() {
    let Some(admin) = pg_admin_url() else { return };
    let env = TestEnv::new().await;
    let src = env.path("src");
    write_source(&src);
    let db = TempDb::create(&admin, "ma_crash").await;
    db.exec("CREATE TABLE accounts (id bigserial PRIMARY KEY, ext text UNIQUE NOT NULL, name text, email text, balance numeric(14,2), joined date);").await;
    env.files_conn("src", &src, "csv", Environment::Production).await;
    env.pg_conn("pg", &db.url, Environment::Production).await;
    let job = prepared_job(&env, "pg", "ext").await;
    let status = worker(&env, &job).env("MA_ENABLE_FAULT_INJECTION", "1").env("MA_FAULT", "after_write:9").status().unwrap();
    assert_eq!(status.code(), Some(86));
    assert_eq!(db.scalar_i64("SELECT count(*) FROM accounts").await, 9 * 500, "batch 9 committed in PostgreSQL but not checkpointed");
    let report = resume_and_verify(&env, &job).await;
    assert_eq!(db.scalar_i64("SELECT count(*) FROM accounts").await, ROWS as i64, "upsert re-write produced no duplicates");
    assert_eq!(report.final_status, FinalStatus::CompletedAndVerified);
    db.drop_db().await;
}

/// Regression: an approval that expires (or is revoked) while a job is not running must not
/// strand the half-finished migration. Without a valid approval nothing more is written; once
/// the operator approves the same plan again, the job continues under the new approval.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_continues_under_a_new_approval_after_its_first_one_ended() {
    let env = TestEnv::new().await;
    let src = env.path("src");
    write_source(&src);
    let out = env.path("out");
    env.files_conn("src", &src, "csv", Environment::Production).await;
    env.files_conn("dst", &out, "jsonl", Environment::Production).await;
    let plan = env.plan("src", "dst", mapping("account_ref"), 500).await.plan_id;
    let consent = OperatorConsent::for_tests("test-operator").unwrap();
    let hour = Duration::from_secs(3600);

    // the first approval lasts one second; the job is created in time but only runs afterwards
    let first = env.app.approve_plan(&plan, &consent, Duration::from_secs(1), false).await.unwrap().claims.approval_id;
    let job = env.app.start_job(&plan, SpawnMode::Inline, Actor::Cli).await.unwrap().job_id;
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let e = env.app.run_worker(&job).await.unwrap_err();
    assert_eq!(e.class, ErrorClass::Policy, "{}", e.message);
    assert!(e.message.contains("approval has expired") && e.message.contains("Approve the plan again"), "{}", e.message);
    let info = env.app.job_status(&job).await.unwrap();
    assert_eq!(info.status, JobStatus::Failed);
    assert!(info.error.as_deref().unwrap_or_default().contains("approval has expired"), "{:?}", info.error);
    assert!(!out.join("accounts.jsonl").exists(), "nothing was written without a valid approval");

    // the operator approves the same plan again: the job runs, and is interrupted after 5 batches
    let second = env.app.approve_plan(&plan, &consent, hour, false).await.unwrap().claims.approval_id;
    env.app.resume_job(&job, SpawnMode::Inline, Actor::Cli).await.unwrap();
    let status = worker(&env, &job).env("MA_ENABLE_FAULT_INJECTION", "1").env("MA_FAULT", "after_commit:5").status().unwrap();
    assert_eq!(status.code(), Some(86), "worker ran under the new approval until fault injection stopped it");
    assert_eq!(env.app.job_status(&job).await.unwrap().stages[0].counters.written(), 5 * 500);

    // that approval is revoked while the job is down: resuming is refused, nothing more is written
    env.app.store.revoke_approval(&second).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await; // the crashed worker's lease expires
    env.app.resume_job(&job, SpawnMode::Inline, Actor::Cli).await.unwrap();
    let e = env.app.run_worker(&job).await.unwrap_err();
    assert!(e.message.contains("revoked") && e.message.contains("Approve the plan again"), "{}", e.message);
    assert_eq!(env.app.job_status(&job).await.unwrap().stages[0].counters.written(), 5 * 500, "no progress without approval");

    // a third approval: the job finishes from its checkpoint, complete and without duplicates
    env.app.approve_plan(&plan, &consent, hour, false).await.unwrap();
    let report = resume_and_verify(&env, &job).await;
    let keys = jsonl_keys(&out.join("accounts.jsonl"), "account_ref");
    assert_eq!(keys.len(), ROWS, "no duplicates");
    assert_eq!(keys.iter().collect::<std::collections::BTreeSet<_>>().len(), ROWS, "no gaps");
    assert_eq!(report.final_status, FinalStatus::CompletedAndVerified);
    let audit = env.app.store.audit_events(200).await.unwrap();
    let renewals: Vec<_> = audit.iter().filter(|e| e.action == "job.approval_renewed").collect();
    assert_eq!(renewals.len(), 2, "each change of approval is on record");
    assert!(renewals.iter().any(|e| e.detail["from"] == serde_json::json!(first) && e.detail["to"] == serde_json::json!(second)));

    // a paused job gives its lease back at once, so it can be resumed or cancelled immediately
    env.files_conn("dst2", &env.path("out2"), "jsonl", Environment::Production).await;
    let job2 = prepared_job(&env, "dst2", "account_ref").await;
    env.app.pause_job(&job2, Actor::Cli).await.unwrap();
    env.app.run_worker(&job2).await.unwrap();
    let (owner, _) = env.app.store.lease_state(&job2).await.unwrap();
    assert!(owner.is_none(), "the lease is released when the worker stops, and stays released");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(env.app.store.lease_state(&job2).await.unwrap().0.is_none(), "no late heartbeat took it back");
    env.app.resume_job(&job2, SpawnMode::Inline, Actor::Cli).await.unwrap();
    env.app.run_worker(&job2).await.unwrap();
    assert_eq!(env.app.job_status(&job2).await.unwrap().status, JobStatus::Completed);
}

// ------------------------------------------------------------------ a destination that cannot answer

/// A small web API with a non-idempotent create and a lookup by `code` that can be made to fail
/// for one code.
#[derive(Default)]
struct ItemsApi {
    items: std::sync::Mutex<Vec<(String, String)>>,
    creates: std::sync::Mutex<std::collections::BTreeMap<String, usize>>,
    broken_lookup: std::sync::Mutex<Option<String>>,
}

async fn serve_items(api: std::sync::Arc<ItemsApi>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let api = api.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 64 * 1024];
                let mut n = 0;
                let (head, body) = loop {
                    match sock.read(&mut buf[n..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(k) => n += k,
                    }
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if n >= end + 4 + len {
                            break (text[..end].to_string(), text[end + 4..end + 4 + len].to_string());
                        }
                    }
                };
                let mut first = head.split_whitespace();
                let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
                let list = |rows: &[(String, String)]| {
                    let rows: Vec<_> = rows.iter().map(|(c, n)| serde_json::json!({"code": c, "name": n})).collect();
                    serde_json::json!({ "items": rows }).to_string()
                };
                let (status, out) = if method == "POST" {
                    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                    let code = v["code"].as_str().unwrap().to_string();
                    *api.creates.lock().unwrap().entry(code.clone()).or_default() += 1;
                    api.items.lock().unwrap().push((code, v["name"].as_str().unwrap_or_default().to_string()));
                    ("201 Created", v.to_string())
                } else if let Some(code) = target.split_once("?code=").map(|(_, c)| c.to_string()) {
                    if api.broken_lookup.lock().unwrap().as_deref() == Some(code.as_str()) {
                        ("400 Bad Request", r#"{"error":"this value cannot be used as a filter"}"#.to_string())
                    } else {
                        let rows: Vec<_> = api.items.lock().unwrap().iter().filter(|(c, _)| *c == code).cloned().collect();
                        ("200 OK", list(&rows))
                    }
                } else {
                    ("200 OK", list(&api.items.lock().unwrap()))
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                    out.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

/// Regression: after a crash between a write and its checkpoint, the destination is asked
/// whether each in-flight record arrived. If that question cannot be answered for one record,
/// the job used to fail there, and again at the same record on every resume. Now that record is
/// set aside for a person to check (never re-sent blindly) and the migration continues.
#[tokio::test(flavor = "multi_thread")]
async fn an_unanswerable_in_flight_check_sets_the_record_aside_and_the_job_continues() {
    let env = TestEnv::new().await;
    let api = std::sync::Arc::new(ItemsApi::default());
    let base = serve_items(api.clone()).await;
    let text = |name: &str| FieldDef::new(name, FieldType::String);
    let spec = DeclarativeConnectorSpec {
        spec_version: 1,
        connector_id: "inflight-api".into(),
        version: "1.0.0".into(),
        protocol: SpecProtocol::Rest,
        title: "Items".into(),
        api_version: None,
        base_path: String::new(),
        authentication: AuthSpec::None,
        rate_limit: None,
        resources: vec![ResourceSpec {
            name: "items".into(),
            identity_field: "code".into(),
            list: Some(ListOperation { path: "/items".into(), records_path: "items".into(), pagination: Pagination::None }),
            create: Some(WriteOperation {
                method: HttpMethod::Post,
                path: "/items".into(),
                response_id_path: String::new(),
                idempotent: false,
            }),
            update: None,
            get: None,
            lookup: Some(LookupOperation {
                path: "/items".into(),
                query_param: "code".into(),
                key_field: "code".into(),
                records_path: "items".into(),
            }),
            fields: vec![text("code"), text("name")],
            relationships: vec![],
        }],
        provenance: SpecProvenance { generated_by: "test".into(), generated_at: "now".into(), source_document_sha256: None, notes: vec![] },
    };
    let reference = env.app.register_connector_spec(&serde_json::to_string(&spec).unwrap(), Actor::Cli).await.unwrap();
    env.app
        .add_connection(
            ConnectionConfig {
                name: "dst".into(),
                kind: "rest".into(),
                environment: Environment::Test,
                settings: serde_json::json!({"base_url": base, "allow_private_network": true}),
                credential_ref: None,
                allowed_hosts: vec!["127.0.0.1".into()],
                connector_ref: Some(reference.clone()),
            },
            Actor::Cli,
        )
        .await
        .unwrap();
    let tested = env.app.test_connector("dst", Actor::Cli).await.unwrap();
    assert!(tested.passed, "{:?}", tested.results);
    env.app.activate_connector(&reference, &OperatorConsent::for_tests("test-operator").unwrap()).await.unwrap();

    let src = env.path("src");
    std::fs::write(src.join("items.csv"), "id,code,name\n1,A1,one\n2,A2,two\n3,A3,three\n4,A4,four\n5,A5,five\n6,A6,six\n").unwrap();
    env.files_conn("src", &src, "csv", Environment::Test).await;
    let mapping = MappingSpec {
        mapping_version: 1,
        objects: vec![ObjectMapping {
            source_object: "items".into(),
            target_object: "items".into(),
            target_key_field: Some("code".into()),
            fields: vec![f("code", "code"), f("name", "name")],
            unmapped_fields: UnmappedFieldPolicy::Skip,
            skip_fields: vec![],
            // plain creates: the destination cannot be re-sent to safely
            write_mode: WriteMode::Insert,
        }],
    };
    let plan = env.plan("src", "dst", mapping, 3).await;
    assert!(plan.approvable, "{:?}", plan.blocking_risks);
    env.approve(&plan.plan_id).await;
    let job = env.app.start_job(&plan.plan_id, SpawnMode::Inline, Actor::Cli).await.unwrap().job_id;

    // the worker creates the first three records and dies before its checkpoint
    let status = worker(&env, &job).env("MA_ENABLE_FAULT_INJECTION", "1").env("MA_FAULT", "after_write:1").status().unwrap();
    assert_eq!(status.code(), Some(86));
    assert_eq!(api.items.lock().unwrap().len(), 3, "A1..A3 reached the destination");

    // from now on the destination cannot answer the question for A2
    *api.broken_lookup.lock().unwrap() = Some("A2".into());
    tokio::time::sleep(Duration::from_millis(1500)).await; // crashed worker's lease expires
    env.app.resume_job(&job, SpawnMode::Inline, Actor::Cli).await.unwrap();
    env.app.run_worker(&job).await.expect("the job continues instead of failing at A2 on every resume");

    let info = env.app.job_status(&job).await.unwrap();
    assert_eq!(info.status, JobStatus::Completed);
    assert_eq!(info.stages[0].counters.outcome(OutcomeKind::RequiresReconciliation), 1);
    let creates = api.creates.lock().unwrap().clone();
    assert_eq!(creates.len(), 6, "every record was created");
    assert!(creates.values().all(|n| *n == 1), "and none of them twice, A2 included: {creates:?}");
    let outcomes = env.app.store.record_outcomes(&job, 100).await.unwrap();
    let set_aside: Vec<_> = outcomes.iter().filter(|o| o.2 == OutcomeKind::RequiresReconciliation.as_str()).collect();
    assert_eq!(set_aside.len(), 1, "{outcomes:?}");
    assert!(set_aside[0].3.as_deref().unwrap_or_default().contains("could not be asked whether it arrived"), "{:?}", set_aside[0]);
}

/// A job started the normal way runs in a detached worker process. On Unix that worker gets its
/// own process group, so Ctrl+C in the terminal that started Studio or the CLI (which signals
/// the terminal's whole foreground group) does not stop the migration.
#[tokio::test(flavor = "multi_thread")]
async fn a_detached_worker_runs_apart_from_the_process_that_started_it() {
    let env = TestEnv::new().await;
    let src = env.path("src");
    write_source(&src);
    env.files_conn("src", &src, "csv", Environment::Production).await;
    env.files_conn("dst", &env.path("out"), "jsonl", Environment::Production).await;
    let plan = env.plan("src", "dst", mapping("account_ref"), 500).await;
    env.approve(&plan.plan_id).await;
    let job = env.app.start_job(&plan.plan_id, SpawnMode::Detached, Actor::Cli).await.unwrap().job_id;
    let audit = env.app.store.audit_events(50).await.unwrap();
    let spawned = audit.iter().find(|e| e.action == "job.worker_spawned").expect("a worker process was started");
    let pid = spawned.detail["pid"].as_u64().unwrap() as u32;
    assert_ne!(pid, std::process::id());
    if cfg!(unix) {
        // asked right after the start, while the worker is still working through 20,000 records
        let group_of = |pid: u32| -> Option<u32> {
            let out = Command::new("ps").args(["-o", "pgid=", "-p", &pid.to_string()]).output().ok()?;
            String::from_utf8_lossy(&out.stdout).trim().parse().ok()
        };
        assert_eq!(group_of(pid), Some(pid), "the worker leads its own process group");
        assert_ne!(group_of(pid), group_of(std::process::id()), "and is not in ours");
    }
    let start = Instant::now();
    while !env.app.job_status(&job).await.unwrap().status.is_terminal() && start.elapsed() < Duration::from_secs(120) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(env.app.job_status(&job).await.unwrap().status, JobStatus::Completed);
}
