//! Benchmark suite. Run: `cargo run -p ma-bench --release -- [--quick] [--out benchmarks/RESULTS.md]`
//!
//! Every scenario runs the real application path (plan → approve → execute → verify) and
//! records wall time, throughput, peak memory (process high-water mark) and CPU utilization.
#![allow(unsafe_code)] // OS process-metric FFI and test-only env setup

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use ma_core::{Actor, App, OperatorConsent, PlanRequest, SpawnMode};
use ma_types::*;

mod metrics;

struct Row {
    scenario: String,
    records: u64,
    exec_secs: f64,
    verify_secs: f64,
    rps: f64,
    verify_rps: f64,
    peak_mb: f64,
    cpu_pct: f64,
    status: String,
    errors: u64,
}

fn write_csv(path: &Path, n: usize, refs: Option<usize>) {
    let f = std::fs::File::create(path).unwrap();
    let mut w = std::io::BufWriter::with_capacity(1 << 20, f);
    if refs.is_some() {
        writeln!(w, "id,parent_id,first,last,email,amount,status,created").unwrap();
    } else {
        writeln!(w, "id,first,last,email,amount,status,created,notes").unwrap();
    }
    let statuses = ["A", "I", "P"];
    for i in 1..=n {
        let amount = format!("{}.{:02}", (i * 7919) % 1_000_000, i % 100);
        let created = format!("2023-{:02}-{:02}T{:02}:{:02}:00Z", 1 + i % 12, 1 + i % 28, i % 24, i % 60);
        match refs {
            Some(parents) => writeln!(
                w,
                "{i},{},First{},Last{},u{i}@example.com,{amount},{},{created}",
                1 + (i * 31) % parents,
                i % 1000,
                i % 777,
                statuses[i % 3]
            )
            .unwrap(),
            None => writeln!(
                w,
                "{i},First{},Last{},u{i}@example.com,{amount},{},{created},\"note, with comma {i}\"",
                i % 1000,
                i % 777,
                statuses[i % 3]
            )
            .unwrap(),
        }
    }
    w.flush().unwrap();
}

fn transform_fields(with_ref: bool) -> Vec<FieldMapping> {
    let mut v = vec![
        FieldMapping::accepted("ext_id", Expr::Cast { input: Box::new(Expr::field("id")), to: FieldType::String }),
        FieldMapping::accepted(
            "full_name",
            Expr::Upper { input: Box::new(Expr::Concat { parts: vec![Expr::field("first"), Expr::field("last")], separator: " ".into() }) },
        ),
        FieldMapping::accepted("email", Expr::Lower { input: Box::new(Expr::field("email")) }),
        FieldMapping::accepted("amount_minor", Expr::ScaleDecimal { input: Box::new(Expr::field("amount")), pow: 2 }),
        FieldMapping::accepted(
            "state",
            Expr::EnumMap {
                input: Box::new(Expr::field("status")),
                map: [("A", "active"), ("I", "inactive"), ("P", "pending")]
                    .into_iter()
                    .map(|(a, b)| (a.to_string(), Value::String(b.into())))
                    .collect(),
                on_unmapped: Unmapped::Fail,
            },
        ),
        FieldMapping::accepted("created_at", Expr::field("created")),
    ];
    if with_ref {
        v.push(FieldMapping::accepted("parent_ref", Expr::Ref { input: Box::new(Expr::field("parent_id")), object: "parents".into() }));
    } else {
        v.push(FieldMapping::accepted("notes", Expr::field("notes")));
    }
    v
}

fn om(src: &str, dst: &str, fields: Vec<FieldMapping>) -> ObjectMapping {
    ObjectMapping {
        source_object: src.into(),
        target_object: dst.into(),
        target_key_field: Some("ext_id".into()),
        fields,
        unmapped_fields: UnmappedFieldPolicy::Archive,
        skip_fields: vec![],
        write_mode: WriteMode::Upsert,
    }
}

async fn conn(app: &App, name: &str, dir: &Path, fmt: &str) {
    app.add_connection(
        ConnectionConfig {
            name: name.into(),
            kind: "files".into(),
            environment: Environment::Production,
            settings: serde_json::json!({"path": dir, "format": fmt}),
            credential_ref: None,
            allowed_hosts: vec![],
            connector_ref: None,
        },
        Actor::Cli,
    )
    .await
    .unwrap();
}

async fn prepare(app: &App, src: &str, dst: &str, mapping: MappingSpec, batch: usize) -> String {
    let plan = app
        .create_plan(
            &PlanRequest {
                source: src.into(),
                destination: dst.into(),
                mapping: Some(mapping),
                pairs: vec![],
                options: Some(ExecutionOptions { batch_size: batch, prefetch_batches: 4, verify_sample_size: 50, ..Default::default() }),
            },
            Actor::Cli,
        )
        .await
        .unwrap();
    assert!(plan.approvable, "{:?}", plan.blocking_risks);
    app.approve_plan(&plan.plan_id, &OperatorConsent::for_tests("bench").unwrap(), Duration::from_secs(3600), false).await.unwrap();
    app.start_job(&plan.plan_id, SpawnMode::Inline, Actor::Cli).await.unwrap().job_id
}

async fn measure(app: &App, scenario: &str, job: &str, records: u64) -> Row {
    let cpu0 = metrics::cpu_time();
    let t0 = Instant::now();
    app.run_worker(job).await.unwrap();
    let exec = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let v = app.verify_job(job, Actor::Cli).await.unwrap();
    let verify = t1.elapsed().as_secs_f64();
    let cpu = metrics::cpu_time() - cpu0;
    let report = app.report(job).await.unwrap();
    let errors = report.records_failed + v.discrepancies.len() as u64;
    for d in v.discrepancies.iter().take(4) {
        eprintln!("  discrepancy: {}", d.chars().take(400).collect::<String>());
    }
    if report.final_status != FinalStatus::CompletedAndVerified {
        eprintln!("  outcomes: {:?} archived={} unsupported={:?}", report.outcomes, report.archived_values, report.unsupported_fields);
    }
    let row = Row {
        scenario: scenario.into(),
        records,
        exec_secs: exec,
        verify_secs: verify,
        rps: records as f64 / exec,
        verify_rps: records as f64 / verify,
        peak_mb: metrics::peak_rss_mb(),
        cpu_pct: 100.0 * cpu / ((exec + verify) * metrics::cores() as f64),
        status: report.final_status.to_string(),
        errors,
    };
    eprintln!(
        "{:<42} {:>9} rec  exec {:>7.2}s ({:>9.0} rec/s)  verify {:>6.2}s  peak {:>7.1} MB  cpu {:>5.1}%  {}",
        row.scenario, row.records, row.exec_secs, row.rps, row.verify_secs, row.peak_mb, row.cpu_pct, row.status
    );
    row
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    unsafe {
        std::env::set_var("MA_INSECURE_TEST_APPROVAL", "1");
    }
    let args: Vec<String> = std::env::args().collect();
    let quick = args.iter().any(|a| a == "--quick");
    let out = args.windows(2).find(|w| w[0] == "--out").map(|w| w[1].clone());
    let tmp = tempfile::tempdir().unwrap();
    let app = App::open(&tmp.path().join("home")).await.unwrap();
    let mut rows: Vec<Row> = vec![];

    // ---------------------------------------------------------------- CSV → JSONL at increasing sizes
    let sizes: &[usize] = if quick { &[10_000] } else { &[10_000, 100_000, 1_000_000] };
    for &n in sizes {
        let src = tmp.path().join(format!("src_{n}"));
        let dst = tmp.path().join(format!("dst_{n}"));
        std::fs::create_dir_all(&src).unwrap();
        let tg = Instant::now();
        write_csv(&src.join("rows.csv"), n, None);
        eprintln!(
            "generated {n} rows ({:.1} MB) in {:.1}s",
            std::fs::metadata(src.join("rows.csv")).unwrap().len() as f64 / 1e6,
            tg.elapsed().as_secs_f64()
        );
        conn(&app, &format!("s{n}"), &src, "csv").await;
        conn(&app, &format!("d{n}"), &dst, "jsonl").await;
        let job = prepare(
            &app,
            &format!("s{n}"),
            &format!("d{n}"),
            MappingSpec { mapping_version: 1, objects: vec![om("rows", "rows", transform_fields(false))] },
            5000,
        )
        .await;
        rows.push(measure(&app, &format!("CSV→JSONL, 7 transformed fields, {n}"), &job, n as u64).await);
    }

    // ---------------------------------------------------------------- relationship-heavy
    {
        let (parents, children) = if quick { (1_000, 10_000) } else { (20_000, 200_000) };
        let src = tmp.path().join("rel_src");
        let dst = tmp.path().join("rel_dst");
        std::fs::create_dir_all(&src).unwrap();
        write_csv(&src.join("parents.csv"), parents, None);
        write_csv(&src.join("children.csv"), children, Some(parents));
        conn(&app, "rs", &src, "csv").await;
        conn(&app, "rd", &dst, "jsonl").await;
        let mapping = MappingSpec {
            mapping_version: 1,
            objects: vec![om("parents", "parents", transform_fields(false)), om("children", "children", transform_fields(true))],
        };
        let job = prepare(&app, "rs", "rd", mapping, 5000).await;
        rows.push(
            measure(&app, &format!("relationships: {parents} parents + {children} children"), &job, (parents + children) as u64).await,
        );
    }

    // ---------------------------------------------------------------- concurrent jobs
    {
        let n = if quick { 10_000 } else { 100_000 };
        let mut jobs = vec![];
        for k in 0..3 {
            let src = tmp.path().join(format!("c_src_{k}"));
            let dst = tmp.path().join(format!("c_dst_{k}"));
            std::fs::create_dir_all(&src).unwrap();
            write_csv(&src.join("rows.csv"), n, None);
            conn(&app, &format!("cs{k}"), &src, "csv").await;
            conn(&app, &format!("cd{k}"), &dst, "jsonl").await;
            jobs.push(
                prepare(
                    &app,
                    &format!("cs{k}"),
                    &format!("cd{k}"),
                    MappingSpec { mapping_version: 1, objects: vec![om("rows", "rows", transform_fields(false))] },
                    5000,
                )
                .await,
            );
        }
        let cpu0 = metrics::cpu_time();
        let t = Instant::now();
        let (a, b, c) = tokio::join!(app.run_worker(&jobs[0]), app.run_worker(&jobs[1]), app.run_worker(&jobs[2]));
        a.unwrap();
        b.unwrap();
        c.unwrap();
        let exec = t.elapsed().as_secs_f64();
        let cpu = metrics::cpu_time() - cpu0;
        let mut errors = 0;
        let t1 = Instant::now();
        for j in &jobs {
            errors += app.verify_job(j, Actor::Cli).await.unwrap().discrepancies.len() as u64;
        }
        let verify = t1.elapsed().as_secs_f64();
        let total = 3 * n as u64;
        eprintln!("3 concurrent jobs x {n}: {exec:.2}s ({:.0} rec/s aggregate)", total as f64 / exec);
        rows.push(Row {
            scenario: format!("3 concurrent jobs x {n}"),
            records: total,
            exec_secs: exec,
            verify_secs: verify,
            rps: total as f64 / exec,
            verify_rps: total as f64 / verify,
            peak_mb: metrics::peak_rss_mb(),
            cpu_pct: 100.0 * cpu / (exec * metrics::cores() as f64),
            status: if errors == 0 { "all verified".into() } else { format!("{errors} discrepancies") },
            errors,
        });
    }

    // ---------------------------------------------------------------- interruption + resume correctness
    {
        let n = if quick { 20_000 } else { 200_000 };
        let src = tmp.path().join("i_src");
        let dst = tmp.path().join("i_dst");
        std::fs::create_dir_all(&src).unwrap();
        write_csv(&src.join("rows.csv"), n, None);
        conn(&app, "is", &src, "csv").await;
        conn(&app, "id", &dst, "jsonl").await;
        let job =
            prepare(&app, "is", "id", MappingSpec { mapping_version: 1, objects: vec![om("rows", "rows", transform_fields(false))] }, 2000)
                .await;
        let t = Instant::now();
        let app_ref = &app;
        let jid = job.clone();
        let pauser = async move {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let j = app_ref.job_status(&jid).await.unwrap();
                if j.stages[0].counters.batches >= 5 {
                    app_ref.pause_job(&jid, Actor::Cli).await.unwrap();
                    return j.stages[0].counters.batches;
                }
            }
        };
        let (end, paused_at) = tokio::join!(app.run_worker(&job), pauser);
        let first_end = end.unwrap();
        app.resume_job(&job, SpawnMode::Inline, Actor::Cli).await.unwrap();
        app.run_worker(&job).await.unwrap();
        let exec = t.elapsed().as_secs_f64();
        let v = app.verify_job(&job, Actor::Cli).await.unwrap();
        let lines = std::fs::read_to_string(dst.join("rows.jsonl")).unwrap().lines().count();
        let ok = v.discrepancies.is_empty() && lines == n;
        for d in v.discrepancies.iter().take(4) {
            eprintln!("  discrepancy: {}", d.chars().take(400).collect::<String>());
        }
        eprintln!("interrupt/resume: first run ended {first_end:?} after >= {paused_at} batches; {lines} lines; correct={ok}");
        rows.push(Row {
            scenario: format!("pause after 5 batches + resume, {n}"),
            records: n as u64,
            exec_secs: exec,
            verify_secs: 0.0,
            rps: n as f64 / exec,
            verify_rps: 0.0,
            peak_mb: metrics::peak_rss_mb(),
            cpu_pct: 0.0,
            status: if ok { "resumed, no duplicates/gaps".into() } else { "INCORRECT".into() },
            errors: if ok { 0 } else { 1 },
        });
    }

    // ---------------------------------------------------------------- transformation throughput (in-memory)
    {
        let m = om("rows", "rows", transform_fields(false));
        let consumed = ma_mapping::runtime::consumed_fields(&m);
        let recs: Vec<Record> = (1..=200_000u64)
            .map(|i| {
                Record::new("rows", i.to_string())
                    .with("id", Value::Int(i as i64))
                    .with("first", Value::String(format!("First{}", i % 1000)))
                    .with("last", Value::String(format!("Last{}", i % 777)))
                    .with("email", Value::String(format!("U{i}@Example.com")))
                    .with("amount", Value::Decimal(format!("{}.{:02}", i * 7, i % 100)))
                    .with("status", Value::String(["A", "I", "P"][(i % 3) as usize].into()))
                    .with("created", Value::String("2023-01-01T00:00:00Z".into()))
                    .with("notes", Value::String("n".into()))
            })
            .collect();
        let refs = |_: &str, _: &str| None;
        let ctx = ma_mapping::EvalContext { refs: &refs, extensions: None };
        let t = Instant::now();
        let mut ok = 0u64;
        for r in &recs {
            if ma_mapping::transform_record(&m, None, &consumed, r, &ctx, false).errors.is_empty() {
                ok += 1;
            }
        }
        let secs = t.elapsed().as_secs_f64();
        eprintln!("transformation runtime: {ok} records in {secs:.3}s = {:.0} rec/s (single thread)", ok as f64 / secs);
        rows.push(Row {
            scenario: "transform runtime only (in-memory, 1 thread)".into(),
            records: ok,
            exec_secs: secs,
            verify_secs: 0.0,
            rps: ok as f64 / secs,
            verify_rps: 0.0,
            peak_mb: metrics::peak_rss_mb(),
            cpu_pct: 0.0,
            status: "n/a".into(),
            errors: 200_000 - ok,
        });
    }

    // ---------------------------------------------------------------- report
    let mut md = String::new();
    let _ = writeln!(md, "# Benchmark results\n");
    let _ = writeln!(md, "* Date: {}", chrono::Utc::now().to_rfc3339());
    let _ = writeln!(md, "* Build: release (`cargo run -p ma-bench --release{}`)", if quick { " -- --quick" } else { "" });
    let _ = writeln!(
        md,
        "* Machine: {} — {} logical cores, {:.1} GB RAM, {}",
        metrics::cpu_name(),
        metrics::cores(),
        metrics::total_ram_gb(),
        std::env::consts::OS
    );
    let _ = writeln!(md, "* Storage: local SSD temp directory; state database SQLite (WAL, synchronous=FULL)");
    let _ = writeln!(
        md,
        "* Workload: CSV source → JSON Lines destination through the full plan → approve → execute → verify path; batch size 5000 (2000 for interrupt test); per-batch fsync + atomic SQLite checkpoint; id map persisted for every record.\n"
    );
    let _ = writeln!(
        md,
        "| scenario | records | execute (s) | records/s | verify (s) | verified rec/s | peak RSS (MB, process high-water) | CPU util. | result |"
    );
    let _ = writeln!(md, "|---|---:|---:|---:|---:|---:|---:|---:|---|");
    for r in &rows {
        let _ = writeln!(
            md,
            "| {} | {} | {:.2} | {:.0} | {} | {} | {:.0} | {} | {} |",
            r.scenario,
            r.records,
            r.exec_secs,
            r.rps,
            if r.verify_secs > 0.0 { format!("{:.2}", r.verify_secs) } else { "-".into() },
            if r.verify_rps > 0.0 { format!("{:.0}", r.verify_rps) } else { "-".into() },
            r.peak_mb,
            if r.cpu_pct > 0.0 { format!("{:.0}%", r.cpu_pct) } else { "-".into() },
            r.status
        );
    }
    let _ = writeln!(md, "\nPeak RSS is the process high-water mark at the time the row was recorded (cumulative across earlier rows).");
    println!("{md}");
    if let Some(p) = out {
        std::fs::write(&p, &md).unwrap();
        eprintln!("written to {p}");
    }
    let failures: u64 = rows.iter().map(|r| r.errors).sum();
    if failures > 0 {
        eprintln!("benchmark detected {failures} correctness problem(s)");
        std::process::exit(2);
    }
}
