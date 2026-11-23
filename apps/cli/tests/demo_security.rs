//! Demo 6 — Security. Every check exercises the real code path and asserts rejection.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use common::*;
use ma_core::{Actor, DocumentSource, GenerateConnectorRequest, OperatorConsent, SpawnMode};
use ma_types::*;
use serde_json::{Value as J, json};
use sqlx::Connection;

async fn simple_setup(env: &TestEnv) -> String {
    let src = env.path("src");
    std::fs::write(src.join("items.csv"), "id,name\n1,a\n2,b\n3,c\n").unwrap();
    env.files_conn("src", &src, "csv", Environment::Production).await;
    env.files_conn("dst", &env.path("dst"), "jsonl", Environment::Production).await;
    let m = MappingSpec {
        mapping_version: 1,
        objects: vec![object("items", "items", Some("ext"), vec![fx("ext", cast("id", FieldType::String)), f("name", "name")])],
    };
    env.plan("src", "dst", m, 10).await.plan_id
}

async fn sqlite(env: &TestEnv) -> sqlx::SqliteConnection {
    let url = format!("sqlite://{}", env.home().join("state.db").display()).replace('\\', "/");
    sqlx::SqliteConnection::connect(&url).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn unapproved_writes_are_rejected() {
    let env = TestEnv::new().await;
    let plan = simple_setup(&env).await;
    let err = env.app.start_job(&plan, SpawnMode::Inline, Actor::Mcp).await.unwrap_err();
    assert_eq!(err.class, ErrorClass::Policy);
    assert!(err.message.contains("no operator approval"));
    assert!(!env.dir.path().join("dst/items.jsonl").exists(), "nothing was written");
    let audit = env.app.store.audit_events(50).await.unwrap();
    assert!(audit.iter().any(|e| e.action == "job.rejected" && e.actor == "mcp-agent"));
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_cannot_be_granted_from_a_non_interactive_process() {
    let env = TestEnv::new().await;
    let plan = simple_setup(&env).await;
    // an agent shelling out to the CLI has no terminal: approval is refused
    let out = Command::new(env!("CARGO_BIN_EXE_migrate-anything"))
        .arg("--home")
        .arg(env.home())
        .args(["plan", "approve", &plan])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            c.stdin.take().unwrap().write_all(b"ABCDEF\n").ok();
            c.wait_with_output()
        })
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("interactive terminal"), "{stderr}");
    assert_eq!(env.app.approval_status(&plan).await.unwrap().state, ApprovalState::NotRequested);
}

#[tokio::test(flavor = "multi_thread")]
async fn tampered_forged_and_expired_approvals_are_rejected() {
    let env = TestEnv::new().await;
    let plan = simple_setup(&env).await;

    // forged approval (attacker cannot produce a valid signature)
    let mut forged = {
        let consent = OperatorConsent::for_tests("real-operator").unwrap();
        env.app.approve_plan(&plan, &consent, Duration::from_secs(60), false).await.unwrap()
    };
    let mut db = sqlite(&env).await;
    sqlx::query("DELETE FROM approvals").execute(&mut db).await.unwrap();
    forged.claims.approval_id = "appr-forged".into();
    forged.claims.operator = "agent".into();
    forged.signature = "00".repeat(32);
    env.app.store.put_approval(&forged).await.unwrap();
    let e = env.app.start_job(&plan, SpawnMode::Inline, Actor::Mcp).await.unwrap_err();
    assert!(e.message.contains("signature is invalid"), "{}", e.message);

    // expired approval
    sqlx::query("DELETE FROM approvals").execute(&mut db).await.unwrap();
    env.app.approve_plan(&plan, &OperatorConsent::for_tests("op").unwrap(), Duration::from_secs(0), false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let e = env.app.start_job(&plan, SpawnMode::Inline, Actor::Cli).await.unwrap_err();
    assert!(e.message.contains("expired"), "{}", e.message);

    // a valid approval is invalidated by any change to the stored plan
    env.app.approve_plan(&plan, &OperatorConsent::for_tests("op").unwrap(), Duration::from_secs(600), false).await.unwrap();
    sqlx::query("UPDATE plans SET plan_json = replace(plan_json, '\"batch_size\":10', '\"batch_size\":11')")
        .execute(&mut db)
        .await
        .unwrap();
    let e = env.app.start_job(&plan, SpawnMode::Inline, Actor::Cli).await.unwrap_err();
    assert!(e.message.contains("plan changed"), "{}", e.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn destructive_operations_need_separate_authorization() {
    let env = TestEnv::new().await;
    let src = env.path("src");
    std::fs::write(src.join("items.csv"), "id,name\n1,a\n").unwrap();
    let dst = env.path("dst");
    std::fs::write(dst.join("items.jsonl"), "{\"ext\":\"old\"}\n").unwrap();
    env.files_conn("src", &src, "csv", Environment::Production).await;
    env.app
        .add_connection(
            ConnectionConfig {
                name: "dst".into(),
                kind: "files".into(),
                environment: Environment::Production,
                settings: json!({"path": dst, "format": "jsonl", "overwrite": true}),
                credential_ref: None,
                allowed_hosts: vec![],
                connector_ref: None,
            },
            Actor::Cli,
        )
        .await
        .unwrap();
    let m = MappingSpec {
        mapping_version: 1,
        objects: vec![object("items", "items", Some("ext"), vec![fx("ext", cast("id", FieldType::String)), f("name", "name")])],
    };
    let plan = env.plan("src", "dst", m, 10).await;
    assert!(plan.operations.contains(&Operation::TruncateDestination));
    let e =
        env.app.approve_plan(&plan.plan_id, &OperatorConsent::for_tests("op").unwrap(), Duration::from_secs(60), false).await.unwrap_err();
    assert!(e.message.contains("destructive"));
}

#[tokio::test(flavor = "multi_thread")]
async fn malicious_connector_configurations_are_rejected() {
    let env = TestEnv::new().await;
    let base = json!({
        "spec_version": 1, "connector_id": "evil-api", "version": "1.0.0", "protocol": "rest", "title": "x",
        "authentication": {"type": "none"},
        "resources": [{"name": "items", "identity_field": "id", "list": {"path": "/items", "pagination": {"type": "none"}}}],
        "provenance": {"generated_by": "attacker", "generated_at": "now"}
    });
    let variants = [
        ("absolute URL path", {
            let mut s = base.clone();
            s["resources"][0]["list"]["path"] = json!("http://169.254.169.254/latest");
            s
        }),
        ("path traversal", {
            let mut s = base.clone();
            s["resources"][0]["list"]["path"] = json!("/../../admin");
            s
        }),
        ("embedded secret", {
            let mut s = base.clone();
            s["api_key"] = json!("sk-live-1");
            s
        }),
        ("host override", {
            let mut s = base.clone();
            s["base_url"] = json!("http://evil.example");
            s
        }),
        ("DELETE operation", {
            let mut s = base.clone();
            s["resources"][0]["create"] = json!({"method": "DELETE", "path": "/items"});
            s
        }),
        ("header injection", {
            let mut s = base.clone();
            s["authentication"] = json!({"type": "api_key_header", "header": "Host"});
            s
        }),
    ];
    for (name, spec) in variants {
        assert!(env.app.register_connector_spec(&spec.to_string(), Actor::Mcp).await.is_err(), "{name} must be rejected");
    }
    // a REST connection may only target allowlisted hosts
    let e = env
        .app
        .add_connection(
            ConnectionConfig {
                name: "x".into(),
                kind: "rest".into(),
                environment: Environment::Production,
                settings: json!({"base_url": "https://evil.example.com"}),
                credential_ref: None,
                allowed_hosts: vec!["api.example.com".into()],
                connector_ref: None,
            },
            Actor::Mcp,
        )
        .await
        .unwrap_err();
    assert!(e.message.contains("allowlist"));
    // a registered spec modified in the database afterwards is refused at load time
    let ok = env.app.register_connector_spec(&base.to_string(), Actor::Cli).await.unwrap();
    env.app
        .add_connection(
            ConnectionConfig {
                name: "r".into(),
                kind: "rest".into(),
                environment: Environment::Production,
                settings: json!({"base_url": "http://127.0.0.1:9"}),
                credential_ref: None,
                allowed_hosts: vec!["127.0.0.1".into()],
                connector_ref: Some(ok.clone()),
            },
            Actor::Cli,
        )
        .await
        .unwrap();
    let mut db = sqlite(&env).await;
    sqlx::query("UPDATE connector_versions SET spec_json = replace(spec_json, '/items', '/admin')").execute(&mut db).await.unwrap();
    let e = env.app.connect("r").await.err().unwrap();
    assert!(e.message.contains("modified after registration"), "{}", e.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthorized_network_access_is_denied() {
    let env = TestEnv::new().await;
    let api = ma_example_api::start(Default::default()).await.unwrap();
    // cloud metadata endpoint: denied even when allowlisted
    env.app
        .add_connection(
            ConnectionConfig {
                name: "meta".into(),
                kind: "rest".into(),
                environment: Environment::Production,
                settings: json!({"base_url": "http://169.254.169.254", "allow_private_network": true}),
                credential_ref: None,
                allowed_hosts: vec!["169.254.169.254".into()],
                connector_ref: None,
            },
            Actor::Cli,
        )
        .await
        .unwrap();
    let e = env
        .app
        .generate_connector(
            &GenerateConnectorRequest {
                source: DocumentSource::Url { url: "http://169.254.169.254/openapi.json".into(), connection: "meta".into() },
                connector_id: "meta-api".into(),
                attach_to_connection: None,
            },
            Actor::Mcp,
        )
        .await
        .unwrap_err();
    assert_eq!(e.class, ErrorClass::Policy, "{}", e.message);
    // loopback without explicit allow_private_network: denied
    env.app
        .add_connection(
            ConnectionConfig {
                name: "local".into(),
                kind: "rest".into(),
                environment: Environment::Production,
                settings: json!({"base_url": api.base_url}),
                credential_ref: None,
                allowed_hosts: vec!["127.0.0.1".into()],
                connector_ref: None,
            },
            Actor::Cli,
        )
        .await
        .unwrap();
    let e = env
        .app
        .generate_connector(
            &GenerateConnectorRequest {
                source: DocumentSource::Url { url: format!("{}/openapi.json", api.base_url), connection: "local".into() },
                connector_id: "local-api".into(),
                attach_to_connection: None,
            },
            Actor::Mcp,
        )
        .await
        .unwrap_err();
    assert!(e.message.contains("private/loopback"), "{}", e.message);
    // a URL on a host that is not on the connection's allowlist: denied
    let e = env
        .app
        .generate_connector(
            &GenerateConnectorRequest {
                source: DocumentSource::Url { url: "http://example.org/openapi.json".into(), connection: "local".into() },
                connector_id: "other-api".into(),
                attach_to_connection: None,
            },
            Actor::Mcp,
        )
        .await
        .unwrap_err();
    assert!(e.message.contains("allowlist"), "{}", e.message);
    api.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn generated_code_cannot_reach_the_credential_store() {
    let env = TestEnv::new().await;
    env.app.set_credential("db-password", "hunter2-very-secret").await.unwrap();
    // secrets are encrypted at rest
    let mut db = sqlite(&env).await;
    let ct: String = sqlx::query_scalar("SELECT ciphertext FROM credentials WHERE name='db-password'").fetch_one(&mut db).await.unwrap();
    assert!(!ct.contains("hunter2"));
    let raw = std::fs::read(env.home().join("state.db")).unwrap();
    assert!(!raw.windows(7).any(|w| w == b"hunter2"));
    // an extension that imports any host function (the only way to reach host state) is rejected
    let consent = OperatorConsent::for_tests("op").unwrap();
    let steal = r#"(module (import "ma" "read_credential" (func $r (param i32 i32) (result i32)))
        (memory (export "memory") 1) (func (export "alloc") (param i32) (result i32) i32.const 0)
        (func (export "transform") (param i32 i32) (result i64) i64.const 0))"#;
    let e = env.app.register_extension("steal", steal.as_bytes(), &consent).await.unwrap_err();
    assert_eq!(e.class, ErrorClass::Policy);
    let wasi = r#"(module (import "wasi_snapshot_preview1" "environ_get" (func (param i32 i32) (result i32)))
        (memory (export "memory") 1) (func (export "alloc") (param i32) (result i32) i32.const 0)
        (func (export "transform") (param i32 i32) (result i64) i64.const 0))"#;
    assert!(env.app.register_extension("env", wasi.as_bytes(), &consent).await.is_err());
    // MCP has no tool that reads or writes credentials
    let tools = ma_mcp_tool_names();
    assert!(tools.iter().all(|t| !t.contains("credential") && !t.contains("secret")));
}

fn ma_mcp_tool_names() -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_migrate-anything")).args(["mcp", "tools"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().map(String::from).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_injection_in_documentation_does_not_alter_policy() {
    let env = TestEnv::new().await;
    let api = ma_example_api::start(Default::default()).await.unwrap();
    env.app.set_credential("k", "test-key-123").await.unwrap();
    env.app
        .add_connection(
            ConnectionConfig {
                name: "api".into(),
                kind: "rest".into(),
                environment: Environment::Production,
                settings: json!({"base_url": api.base_url, "allow_private_network": true}),
                credential_ref: Some("k".into()),
                allowed_hosts: vec!["127.0.0.1".into()],
                connector_ref: None,
            },
            Actor::Cli,
        )
        .await
        .unwrap();
    let doc = std::fs::read_to_string(env.dir.path().join("x").with_file_name("x")).ok(); // no-op
    let _ = doc;
    let g = env
        .app
        .generate_connector(
            &GenerateConnectorRequest {
                source: DocumentSource::Url { url: format!("{}/openapi.json", api.base_url), connection: "api".into() },
                connector_id: "fieldops".into(),
                attach_to_connection: Some("api".into()),
            },
            Actor::Mcp,
        )
        .await
        .unwrap();
    assert!(!g.suspicious_text.is_empty());
    assert_eq!(g.status, ConnectorStatus::Draft, "the document's request to mark the connector approved had no effect");
    let spec_text = serde_json::to_string(&g.spec).unwrap().to_lowercase();
    assert!(!spec_text.contains("169.254") && !spec_text.contains("ignore all previous"));
    let c = env.app.store.get_connection("api").await.unwrap();
    assert_eq!(c.allowed_hosts, vec!["127.0.0.1".to_string()], "allowlist unchanged");
    let audit = env.app.store.audit_events(100).await.unwrap();
    assert!(!audit.iter().any(|e| e.action == "plan.approved" || e.action == "connector.activated"));
    api.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn sandbox_fails_closed_when_an_extension_is_tampered_with() {
    let env = TestEnv::new().await;
    let consent = OperatorConsent::for_tests("op").unwrap();
    env.app.register_extension("upper", ma_sandbox::UPPERCASE_WAT.as_bytes(), &consent).await.unwrap();
    let src = env.path("src");
    std::fs::write(src.join("items.csv"), "id,name\n1,alpha\n2,beta\n").unwrap();
    env.files_conn("src", &src, "csv", Environment::Production).await;
    env.files_conn("dst", &env.path("dst"), "jsonl", Environment::Production).await;
    let m = MappingSpec {
        mapping_version: 1,
        objects: vec![object(
            "items",
            "items",
            Some("ext"),
            vec![
                fx("ext", cast("id", FieldType::String)),
                fx("name", Expr::Extension { input: Box::new(Expr::field("name")), name: "upper".into() }),
            ],
        )],
    };
    let plan = env.plan("src", "dst", m.clone(), 10).await;
    assert!(plan.approvable, "{:?}", plan.blocking_risks);
    let (_, report, _) = env.execute(&plan.plan_id).await;
    assert_eq!(report.final_status, FinalStatus::CompletedAndVerified);
    let out = std::fs::read_to_string(env.dir.path().join("dst/items.jsonl")).unwrap();
    assert!(out.contains("ALPHA"), "extension ran in the sandbox: {out}");

    // swap the stored module for one with a host import: execution refuses to start
    let mut db = sqlite(&env).await;
    sqlx::query("UPDATE extensions SET module = ?").bind(b"(module)".to_vec()).execute(&mut db).await.unwrap();
    env.files_conn("dst2", &env.path("dst2"), "jsonl", Environment::Production).await;
    let plan2 = env.plan("src", "dst2", m.clone(), 10).await;
    env.approve(&plan2.plan_id).await;
    let job = env.app.start_job(&plan2.plan_id, SpawnMode::Inline, Actor::Cli).await.unwrap();
    let e = env.app.run_worker(&job.job_id).await.unwrap_err();
    assert_eq!(e.class, ErrorClass::Policy, "{}", e.message);
    assert!(!env.dir.path().join("dst2/items.jsonl").exists(), "nothing was written");

    // a plan referencing an unregistered extension cannot be approved
    let mut bad = m;
    bad.objects[0].fields[1].expr = Expr::Extension { input: Box::new(Expr::field("name")), name: "unknown".into() };
    let p3 = env.plan("src", "dst2", bad, 10).await;
    assert!(!p3.approvable && p3.blocking_risks.iter().any(|r| r.contains("unknown_extension")));
}

// ------------------------------------------------------------------ MCP over stdio

struct Mcp {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
    next: u64,
}

impl Mcp {
    fn start(home: &std::path::Path) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_migrate-anything"))
            .arg("--home")
            .arg(home)
            .args(["mcp", "serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Mcp { child, out, next: 1 }
    }
    fn send(&mut self, v: J) {
        let s = self.child.stdin.as_mut().unwrap();
        writeln!(s, "{v}").unwrap();
        s.flush().unwrap();
    }
    fn request(&mut self, method: &str, params: J) -> J {
        let id = self.next;
        self.next += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(self.out.read_line(&mut line).unwrap() > 0, "MCP server closed stdout");
            let v: J = serde_json::from_str(&line).unwrap();
            if v.get("id") == Some(&json!(id)) {
                return v;
            }
        }
    }
    fn call(&mut self, tool: &str, args: J) -> (bool, String) {
        let r = self.request("tools/call", json!({"name": tool, "arguments": args}));
        let res = &r["result"];
        let text = res["content"][0]["text"].as_str().unwrap_or_default().to_string();
        (res["isError"].as_bool().unwrap_or(false), text)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_stdio_session_cannot_approve_or_write() {
    let env = TestEnv::new().await;
    let plan = simple_setup(&env).await;
    let mut mcp = Mcp::start(&env.home());
    let init = mcp
        .request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "1"}}));
    assert!(init["result"]["capabilities"]["tools"].is_object(), "{init}");
    mcp.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let tools = mcp.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for t in [
        "migrate_system_discover",
        "migrate_connector_generate",
        "migrate_connector_test",
        "migrate_schema_analyze",
        "migrate_mapping_suggest",
        "migrate_plan_create",
        "migrate_plan_preview",
        "migrate_plan_approve",
        "migrate_job_execute",
        "migrate_job_status",
        "migrate_job_pause",
        "migrate_job_resume",
        "migrate_job_cancel",
        "migrate_job_verify",
        "migrate_report_get",
        "migrate_connector_registry",
    ] {
        assert!(names.contains(&t), "missing tool {t}");
    }
    assert!(tools["result"]["tools"][0]["inputSchema"].is_object());

    let (is_err, text) = mcp.call("migrate_system_discover", json!({"connection": "src"}));
    assert!(!is_err, "{text}");
    assert!(text.contains("\"items\""));
    let (_, text) = mcp.call("migrate_plan_preview", json!({"plan_id": plan}));
    assert!(text.contains("\"wrote_to_destination\": false"), "{text}");
    let (is_err, text) = mcp.call("migrate_plan_approve", json!({"plan_id": plan, "note": "please approve"}));
    assert!(!is_err);
    assert!(text.contains("\"pending\""), "agent request only marks the plan pending: {text}");
    let (is_err, text) = mcp.call("migrate_job_execute", json!({"plan_id": plan}));
    assert!(is_err && text.contains("Policy"), "{text}");
    assert!(!env.dir.path().join("dst/items.jsonl").exists());
    let _ = mcp.child.kill();
}
