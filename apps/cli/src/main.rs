//! `migrate-anything` — CLI, MCP server entry point and job worker. A thin adapter over
//! `ma_core::App`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use ma_core::{Actor, App, DocumentSource, GenerateConnectorRequest, OperatorConsent, PlanRequest, SpawnMode};
use ma_types::{ConnectionConfig, Environment, ExecutionOptions, MaError, MappingSpec};

#[derive(Parser)]
#[command(name = "migrate-anything", version, about = "Universal AI-powered data migration. Any source. Any destination.")]
struct Cli {
    /// Workspace directory (default: $MA_HOME or ~/.migrate-anything)
    #[arg(long, global = true, env = "MA_HOME")]
    home: Option<PathBuf>,
    /// Machine-readable JSON output
    #[arg(long, global = true)]
    json: bool,
    /// Without a command, Migrate Anything Studio (the web interface) opens.
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open Migrate Anything Studio, the web interface (this is the default)
    Ui {
        /// Port on 127.0.0.1 (default 4747, or any free port if taken)
        #[arg(long)]
        port: Option<u16>,
        /// Do not open the browser automatically
        #[arg(long)]
        no_open: bool,
    },
    /// Open Studio with a ready-made demo migration (made-up data, nothing leaves this computer)
    Demo {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        no_open: bool,
    },
    /// Verify the installation (state database, keys, sandbox)
    Doctor,
    /// MCP server
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// Connections to systems
    Connections {
        #[command(subcommand)]
        cmd: ConnCmd,
    },
    /// Encrypted credentials (operator only; never exposed over MCP)
    Credentials {
        #[command(subcommand)]
        cmd: CredCmd,
    },
    /// System discovery and schema analysis
    Systems {
        #[command(subcommand)]
        cmd: SysCmd,
    },
    /// Connector registry and the self-building connector factory
    Connectors {
        #[command(subcommand)]
        cmd: ConnectorCmd,
    },
    /// Sandboxed WebAssembly transform extensions
    Extensions {
        #[command(subcommand)]
        cmd: ExtCmd,
    },
    /// Mapping suggestions
    Mapping {
        #[command(subcommand)]
        cmd: MapCmd,
    },
    /// Migration plans, dry runs and approval
    Plan {
        #[command(subcommand)]
        cmd: PlanCmd,
    },
    /// Migration jobs
    Jobs {
        #[command(subcommand)]
        cmd: JobCmd,
    },
    /// Verify a job's results against source and destination
    Verify { job: String },
    /// Print the migration report of a job
    Report { job: String },
    /// Show archived values and records that were not written
    Exceptions {
        job: String,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Show the audit log
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Internal: job worker process
    #[command(hide = true)]
    Worker {
        #[command(subcommand)]
        cmd: WorkerCmd,
    },
}

#[derive(Subcommand)]
enum McpCmd {
    /// Serve MCP over stdio (for Claude Code, Cursor, ...)
    Serve,
    /// List the MCP tools
    Tools,
}

#[derive(Subcommand)]
enum ConnCmd {
    /// Add or update a connection
    Add {
        name: String,
        /// files | postgres | rest
        #[arg(long)]
        kind: String,
        /// test | staging | production
        #[arg(long, default_value = "production")]
        env: String,
        /// Kind-specific settings as JSON, e.g. '{"path":"./data","format":"csv"}'
        #[arg(long)]
        settings: String,
        /// Credential name (see `credentials set`)
        #[arg(long)]
        credential_ref: Option<String>,
        /// Host this connection may contact (repeatable)
        #[arg(long = "allow-host")]
        allow_hosts: Vec<String>,
    },
    List,
}

#[derive(Subcommand)]
enum CredCmd {
    /// Store a secret. Reads it from the terminal (hidden is not supported; prefer --from-env in CI)
    Set {
        name: String,
        /// Read the secret from this environment variable instead of stdin
        #[arg(long)]
        from_env: Option<String>,
    },
    List,
}

#[derive(Subcommand)]
enum SysCmd {
    Discover {
        connection: String,
    },
    Analyze {
        connection: String,
        #[arg(long)]
        objects: Vec<String>,
        #[arg(long, default_value_t = 200)]
        sample: usize,
        /// Include example values (local output only)
        #[arg(long)]
        examples: bool,
    },
}

#[derive(Subcommand)]
enum ConnectorCmd {
    List,
    /// Generate a declarative connector from an OpenAPI document
    Generate {
        #[arg(long)]
        connection: String,
        #[arg(long = "id")]
        connector_id: String,
        #[arg(long)]
        openapi_url: Option<String>,
        #[arg(long)]
        openapi_file: Option<String>,
    },
    /// Run contract tests for the connection's generated connector
    Test {
        #[arg(long)]
        connection: String,
    },
    /// Operator: activate a tested connector (interactive confirmation)
    Activate {
        reference: String,
    },
    /// Register a hand-written declarative spec (JSON file)
    RegisterSpec {
        file: PathBuf,
    },
}

#[derive(Subcommand)]
enum ExtCmd {
    /// Operator: validate, self-test and activate a WebAssembly extension (.wasm or .wat)
    Register { name: String, file: PathBuf },
}

#[derive(Subcommand)]
enum MapCmd {
    Suggest {
        #[arg(long)]
        source: String,
        #[arg(long = "dest")]
        destination: String,
        /// Write the draft mapping spec to this file for review/editing
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum PlanCmd {
    Create {
        #[arg(long)]
        source: String,
        #[arg(long = "dest")]
        destination: String,
        /// Reviewed mapping spec (JSON). Default: heuristic suggestion
        #[arg(long)]
        mapping: Option<PathBuf>,
        #[arg(long)]
        batch_size: Option<usize>,
    },
    /// Show a plan; --dry-run performs a read-only dry run
    Preview {
        plan: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value_t = 100)]
        sample: usize,
    },
    /// Operator: approve a plan (interactive terminal + confirmation code)
    Approve {
        plan: String,
        #[arg(long, default_value_t = 24)]
        ttl_hours: u64,
        #[arg(long)]
        allow_destructive: bool,
    },
    List,
}

#[derive(Subcommand)]
enum JobCmd {
    /// Start an approved plan
    Start {
        plan: String,
        /// Run in this process instead of a detached worker
        #[arg(long)]
        foreground: bool,
    },
    Status {
        job: String,
    },
    List,
    Pause {
        job: String,
    },
    Resume {
        job: String,
        #[arg(long)]
        foreground: bool,
    },
    Cancel {
        job: String,
    },
}

#[derive(Subcommand)]
enum WorkerCmd {
    Run {
        #[arg(long)]
        job: String,
    },
}

/// A session that can take approvals exists only when a person started us from a terminal.
fn interactive_session() -> Option<ma_core::InteractiveSession> {
    let session = ma_core::InteractiveSession::from_terminal().ok();
    #[cfg(feature = "test-approval")]
    let session = session.or_else(|| ma_core::InteractiveSession::for_tests().ok());
    session
}

async fn studio(app: App, port: Option<u16>, no_open: bool, fragment: &str) -> Result<()> {
    let session = interactive_session();
    let ui = ma_ui::serve(Arc::new(app), ma_ui::UiOptions { port: port.unwrap_or(0), session })
        .await
        .context("could not start Migrate Anything Studio (is the port in use?)")?;
    let url = format!("{}{fragment}", ui.url);
    eprintln!();
    eprintln!("  Migrate Anything Studio is running.");
    eprintln!();
    eprintln!("  Open:       {url}");
    if ui.approvals_enabled() {
        eprintln!("  Approvals:  when you approve in the browser, a one-time code appears in this window.");
    } else {
        eprintln!("  Approvals:  disabled here (not an interactive terminal). Use `migrate-anything plan approve <plan>`.");
    }
    eprintln!("  Keep this window open. Press Ctrl+C to stop.");
    eprintln!();
    if !no_open && !ma_ui::open_in_browser(&url) {
        eprintln!("  (Could not open the browser automatically; copy the link above.)");
    }
    ui.wait().await.context("Studio stopped unexpectedly")
}

fn ma(e: MaError) -> anyhow::Error {
    anyhow!("{:?}: {}", e.class, e.message)
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("MA_LOG").unwrap_or_else(|_| "warn".into()))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let home = cli.home.clone().unwrap_or_else(App::default_home);
    let cmd = cli.cmd.unwrap_or(Cmd::Ui { port: None, no_open: false });
    if let Cmd::Mcp { cmd: McpCmd::Tools } = &cmd {
        for t in ma_mcp::tool_names() {
            println!("{t}");
        }
        return Ok(());
    }
    let app = App::open(&home).await.map_err(ma).with_context(|| format!("opening workspace {}", home.display()))?;
    let json = cli.json;
    match cmd {
        Cmd::Ui { port, no_open } => studio(app, port, no_open, "").await?,
        Cmd::Demo { port, no_open } => studio(app, port, no_open, "#/demo").await?,
        Cmd::Doctor => {
            let r = app.doctor().await.map_err(ma)?;
            if json {
                print_json(&r)?;
            } else {
                for c in &r.checks {
                    println!("{} {:<16} {}", if c.ok { "ok  " } else { "FAIL" }, c.name, c.detail);
                }
                println!("\n{}", if r.ok { "Installation verified." } else { "Problems found." });
            }
            if !r.ok {
                bail!("doctor found problems");
            }
        }
        Cmd::Mcp { cmd: McpCmd::Serve } => {
            ma_mcp::serve_stdio(Arc::new(app)).await.map_err(|e| anyhow!("{e}"))?;
        }
        Cmd::Mcp { cmd: McpCmd::Tools } => unreachable!(),
        Cmd::Connections { cmd } => match cmd {
            ConnCmd::Add { name, kind, env, settings, credential_ref, allow_hosts } => {
                let environment: Environment = env.parse().map_err(|e: String| anyhow!(e))?;
                let settings: serde_json::Value = serde_json::from_str(&settings).context("--settings must be JSON")?;
                let cfg = ConnectionConfig {
                    name: name.clone(),
                    kind,
                    environment,
                    settings,
                    credential_ref,
                    allowed_hosts: allow_hosts,
                    connector_ref: None,
                };
                app.add_connection(cfg, Actor::Cli).await.map_err(ma)?;
                println!("connection `{name}` saved");
            }
            ConnCmd::List => {
                let list = app.store.list_connections().await.map_err(ma)?;
                if json {
                    print_json(&list)?;
                } else {
                    for c in list {
                        println!(
                            "{:<20} {:<9} {:<11} {}{}",
                            c.name,
                            c.kind,
                            c.environment,
                            c.settings,
                            c.connector_ref.map(|r| format!("  connector={r}")).unwrap_or_default()
                        );
                    }
                }
            }
        },
        Cmd::Credentials { cmd } => match cmd {
            CredCmd::Set { name, from_env } => {
                let secret = match from_env {
                    Some(var) => std::env::var(&var).with_context(|| format!("environment variable {var} not set"))?,
                    None => {
                        eprint!("secret for `{name}`: ");
                        let mut s = String::new();
                        std::io::stdin().read_line(&mut s)?;
                        s.trim_end_matches(['\r', '\n']).to_string()
                    }
                };
                if secret.is_empty() {
                    bail!("empty secret");
                }
                app.set_credential(&name, &secret).await.map_err(ma)?;
                println!("credential `{name}` stored (encrypted)");
            }
            CredCmd::List => {
                for n in app.store.list_credentials().await.map_err(ma)? {
                    println!("{n}");
                }
            }
        },
        Cmd::Systems { cmd } => match cmd {
            SysCmd::Discover { connection } => {
                let d = app.discover(&connection, Actor::Cli).await.map_err(ma)?;
                if json {
                    print_json(&d)?;
                } else {
                    println!("{} ({}) {}", d.identity.name, d.identity.kind, d.identity.version.unwrap_or_default());
                    println!("capabilities:");
                    for (k, c) in &d.manifest.capabilities {
                        println!("  {k:<18} {:?}{}", c.evidence, c.detail.as_ref().map(|d| format!(" — {d}")).unwrap_or_default());
                    }
                    println!("objects:");
                    for o in &d.schema.objects {
                        println!(
                            "  {} ({} fields, key {:?}, count {})",
                            o.name,
                            o.fields.len(),
                            o.primary_key,
                            o.estimated_count.map(|n| n.to_string()).unwrap_or("?".into())
                        );
                    }
                    for r in &d.schema.relationships {
                        println!("  rel {}.{} → {}.{} ({:?})", r.from_object, r.from_field, r.to_object, r.to_field, r.evidence);
                    }
                }
            }
            SysCmd::Analyze { connection, objects, sample, examples } => {
                print_json(&app.analyze_schema(&connection, &objects, sample, examples).await.map_err(ma)?.profiles)?;
            }
        },
        Cmd::Connectors { cmd } => match cmd {
            ConnectorCmd::List => {
                let reg = app.connector_registry().await.map_err(ma)?;
                if json {
                    print_json(&reg)?;
                } else {
                    for e in reg {
                        println!(
                            "{:<40} {:<9} {:<9} {} {}",
                            e.reference,
                            e.kind,
                            e.status,
                            e.title,
                            if e.resources.is_empty() { String::new() } else { format!("[{}]", e.resources.join(", ")) }
                        );
                    }
                }
            }
            ConnectorCmd::Generate { connection, connector_id, openapi_url, openapi_file } => {
                let source = match (openapi_url, openapi_file) {
                    (Some(url), None) => DocumentSource::Url { url, connection: connection.clone() },
                    (None, Some(path)) => DocumentSource::File { path },
                    _ => bail!("give exactly one of --openapi-url / --openapi-file"),
                };
                let r = app
                    .generate_connector(
                        &GenerateConnectorRequest { source, connector_id, attach_to_connection: Some(connection) },
                        Actor::Cli,
                    )
                    .await
                    .map_err(ma)?;
                if json {
                    print_json(&r)?;
                } else {
                    println!("generated {} ({}){}", r.connector_ref, r.status, if r.reused_existing { " — reused" } else { "" });
                    for res in &r.spec.resources {
                        println!(
                            "  resource {:<20} identity={} fields={} list={} create={} lookup={}",
                            res.name,
                            res.identity_field,
                            res.fields.len(),
                            res.list.is_some(),
                            res.create.is_some(),
                            res.lookup.is_some()
                        );
                    }
                    for n in &r.notes {
                        println!("  note: {n}");
                    }
                    for s in &r.suspicious_text {
                        println!("  WARNING (ignored instruction-like text): {s}");
                    }
                }
            }
            ConnectorCmd::Test { connection } => {
                let r = app.test_connector(&connection, Actor::Cli).await.map_err(ma)?;
                if json {
                    print_json(&r)?;
                } else {
                    for t in &r.results {
                        println!("{} {:<36} {}", if t.passed { "pass" } else { "FAIL" }, t.name, t.detail);
                    }
                    println!("status: {}", r.status);
                }
            }
            ConnectorCmd::Activate { reference } => {
                let consent =
                    OperatorConsent::from_terminal_challenge(&format!("Activate generated connector {reference} for use in migrations?"))
                        .map_err(ma)?;
                app.activate_connector(&reference, &consent).await.map_err(ma)?;
                println!("connector {reference} activated by {}", consent.operator());
            }
            ConnectorCmd::RegisterSpec { file } => {
                let text = std::fs::read_to_string(&file)?;
                println!("registered {}", app.register_connector_spec(&text, Actor::Cli).await.map_err(ma)?);
            }
        },
        Cmd::Extensions { cmd: ExtCmd::Register { name, file } } => {
            let bytes = std::fs::read(&file)?;
            let consent =
                OperatorConsent::from_terminal_challenge(&format!("Activate WebAssembly extension `{name}` from {}?", file.display()))
                    .map_err(ma)?;
            let sha = app.register_extension(&name, &bytes, &consent).await.map_err(ma)?;
            println!("extension `{name}` active (sha256 {sha})");
        }
        Cmd::Mapping { cmd: MapCmd::Suggest { source, destination, out } } => {
            let s = app.suggest_mapping(&source, &destination, &[]).await.map_err(ma)?;
            if let Some(path) = out {
                std::fs::write(&path, serde_json::to_string_pretty(&s.mapping)?)?;
                eprintln!("draft mapping written to {} ({} field(s) need review)", path.display(), s.needs_review);
            }
            if json {
                print_json(&s)?;
            } else {
                for p in &s.pairings {
                    println!("{} → {} ({:.2}) {}", p.source_object, p.target_object, p.confidence, p.explanation);
                }
                for o in &s.mapping.objects {
                    println!("\n[{} → {}] key={:?}", o.source_object, o.target_object, o.target_key_field);
                    for f in &o.fields {
                        let mut src = vec![];
                        f.expr.source_paths(&mut src);
                        println!("  {:<24} ← {:<24} {:.2} {:?}  {}", f.target, src.join(","), f.confidence, f.status, f.explanation);
                    }
                }
            }
        }
        Cmd::Plan { cmd } => match cmd {
            PlanCmd::Create { source, destination, mapping, batch_size } => {
                let mapping: Option<MappingSpec> = match mapping {
                    Some(p) => {
                        Some(serde_json::from_str(&std::fs::read_to_string(&p)?).context("mapping file is not a valid MappingSpec")?)
                    }
                    None => None,
                };
                let options = batch_size.map(|b| ExecutionOptions { batch_size: b, ..Default::default() });
                let s =
                    app.create_plan(&PlanRequest { source, destination, mapping, pairs: vec![], options }, Actor::Cli).await.map_err(ma)?;
                if json {
                    print_json(&s)?;
                } else {
                    println!("{}", s.markdown);
                    println!("plan id: {}\nhash: {}\napprovable: {}", s.plan_id, s.plan_hash, s.approvable);
                }
            }
            PlanCmd::Preview { plan, dry_run, sample } => {
                let p = app.preview_plan(&plan, dry_run, sample).await.map_err(ma)?;
                if json {
                    print_json(&serde_json::json!({"summary": p.summary, "dry_run": p.dry_run, "approval": p.approval}))?;
                } else {
                    println!("{}", p.summary.markdown);
                    if let Some(d) = &p.dry_run {
                        println!("## Dry run (no destination writes: {})\n", !d.wrote_to_destination);
                        for o in &d.objects {
                            println!(
                                "* {} → {}: sampled {}, ok {}, failing {}, archived fields {:?}",
                                o.source_object, o.target_object, o.sampled, o.transform_ok, o.transform_failed, o.archived_fields
                            );
                            for e in &o.examples {
                                println!("    {e}");
                            }
                        }
                        println!("dry run {}", if d.passed { "PASSED" } else { "found issues" });
                    }
                    println!("approval: {:?} — {}", p.approval.state, p.approval.detail);
                }
            }
            PlanCmd::Approve { plan, ttl_hours, allow_destructive } => {
                let sp = app.get_plan(&plan).await.map_err(ma)?;
                let summary = format!("{}\n\nYou are approving plan {} (sha256 {}).", sp.summary_md, plan, sp.plan_hash);
                let consent = OperatorConsent::from_terminal_challenge(&summary).map_err(ma)?;
                let rec = app.approve_plan(&plan, &consent, Duration::from_secs(ttl_hours * 3600), allow_destructive).await.map_err(ma)?;
                println!("approved: {} (expires {})", rec.claims.approval_id, rec.claims.expires_at.to_rfc3339());
            }
            PlanCmd::List => {
                for (id, hash, at) in app.store.list_plans().await.map_err(ma)? {
                    println!("{id}  {}  {at}", &hash[..16]);
                }
            }
        },
        Cmd::Jobs { cmd } => match cmd {
            JobCmd::Start { plan, foreground } => {
                let mode = if foreground { SpawnMode::Inline } else { SpawnMode::Detached };
                let j = app.start_job(&plan, mode, Actor::Cli).await.map_err(ma)?;
                println!("job {} started", j.job_id);
                if foreground {
                    let end = app.run_worker(&j.job_id).await.map_err(ma)?;
                    println!("job {} ended: {end:?}", j.job_id);
                }
            }
            JobCmd::Status { job } => {
                let j = app.job_status(&job).await.map_err(ma)?;
                if json {
                    print_json(&j)?;
                } else {
                    println!("{} {} (plan {}, attempts {})", j.job_id, j.status.as_str(), j.plan_id, j.attempts);
                    if let Some(e) = &j.error {
                        println!("error: {e}");
                    }
                    for s in &j.stages {
                        println!(
                            "  {:<28} {:<8} read {:>9} written {:>9} batches {:>6} retries {}",
                            s.source_object,
                            s.state,
                            s.counters.read,
                            s.counters.written(),
                            s.counters.batches,
                            s.counters.retries
                        );
                    }
                }
            }
            JobCmd::List => {
                for (id, plan, status, at) in app.list_jobs().await.map_err(ma)? {
                    println!("{id}  {status:<10} {plan}  {at}");
                }
            }
            JobCmd::Pause { job } => println!("{}", app.pause_job(&job, Actor::Cli).await.map_err(ma)?.status.as_str()),
            JobCmd::Cancel { job } => println!("{}", app.cancel_job(&job, Actor::Cli).await.map_err(ma)?.status.as_str()),
            JobCmd::Resume { job, foreground } => {
                let mode = if foreground { SpawnMode::Inline } else { SpawnMode::Detached };
                app.resume_job(&job, mode, Actor::Cli).await.map_err(ma)?;
                if foreground {
                    println!("job {job} ended: {:?}", app.run_worker(&job).await.map_err(ma)?);
                } else {
                    println!("job {job} resumed");
                }
            }
        },
        Cmd::Verify { job } => {
            let v = app.verify_job(&job, Actor::Cli).await.map_err(ma)?;
            if json {
                print_json(&v)?;
            } else {
                for c in &v.checks {
                    println!(
                        "{} {:<26} {:<16} {}",
                        if c.passed { "pass" } else { "FAIL" },
                        c.name,
                        c.object.clone().unwrap_or_default(),
                        c.detail
                    );
                }
                println!("{} discrepancy(ies)", v.discrepancies.len());
            }
        }
        Cmd::Report { job } => {
            let r = app.report(&job).await.map_err(ma)?;
            if json {
                print_json(&r)?;
            } else {
                println!("{}", r.to_text());
            }
        }
        Cmd::Exceptions { job, limit } => print_json(&app.exceptions(&job, limit).await.map_err(ma)?)?,
        Cmd::Audit { limit } => {
            for e in app.store.audit_events(limit).await.map_err(ma)? {
                println!("{} {:<22} {:<26} {} {}", e.at, e.actor, e.action, e.subject.unwrap_or_default(), e.detail);
            }
        }
        Cmd::Worker { cmd: WorkerCmd::Run { job } } => {
            let end = app.run_worker(&job).await.map_err(ma)?;
            eprintln!("worker finished job {job}: {end:?}");
        }
    }
    Ok(())
}
