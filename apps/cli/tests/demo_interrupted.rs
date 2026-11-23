//! Demo 5 — Interrupted migration. Real worker processes are killed mid-run (by fault injection
//! in the worst window — after a destination write, before its checkpoint — and by an external
//! kill), restarted, and the result is verified: no duplicates, no gaps.

mod common;

use std::fmt::Write as _;
use std::process::Command;
use std::time::{Duration, Instant};

use common::*;
use ma_core::{Actor, SpawnMode};
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
