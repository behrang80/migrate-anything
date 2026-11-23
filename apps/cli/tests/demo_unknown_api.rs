//! Demo 4 — Unknown system. A local API ("FieldOps Legacy Service") with an OpenAPI document and
//! no preinstalled connector. Migrate Anything discovers it, generates a declarative connector,
//! validates and contract-tests it, reads its records (through rate limiting), migrates them to a
//! file destination, then writes them into a second, empty instance of the same API through the
//! generated connector, and verifies both results.

mod common;

use common::*;
use ma_core::{Actor, DocumentSource, GenerateConnectorRequest, OperatorConsent};
use ma_types::*;
use serde_json::json;

async fn rest_conn(env: &TestEnv, name: &str, base: &str, environment: Environment) {
    let host = url_host(base);
    env.app
        .add_connection(
            ConnectionConfig {
                name: name.into(),
                kind: "rest".into(),
                environment,
                settings: json!({"base_url": base, "allow_private_network": true}),
                credential_ref: Some("fieldops-key".into()),
                allowed_hosts: vec![host],
                connector_ref: None,
            },
            Actor::Cli,
        )
        .await
        .unwrap();
}

fn url_host(u: &str) -> String {
    u.split("://").nth(1).unwrap().split([':', '/']).next().unwrap().to_string()
}

fn api_mapping() -> MappingSpec {
    MappingSpec {
        mapping_version: 1,
        objects: vec![
            object(
                "technicians",
                "technicians",
                Some("tech_id"),
                vec![
                    f("tech_id", "tech_id"),
                    f("full_name", "full_name"),
                    f("email", "email"),
                    f("skill_level", "skill_level"),
                    f("hourly_rate", "hourly_rate"),
                    f("active", "active"),
                    f("hired_on", "hired_on"),
                    f("region_code", "region_code"),
                    f("x_badge_color", "x_badge_color"),
                ],
            ),
            object(
                "work_orders",
                "work_orders",
                Some("wo_number"),
                vec![
                    f("wo_number", "wo_number"),
                    f("title", "title"),
                    f("status", "status"),
                    reff("assigned_tech", "assigned_tech", "technicians"),
                    f("priority", "priority"),
                    f("estimated_hours", "estimated_hours"),
                    f("site", "site"),
                    f("opened_at", "opened_at"),
                    f("tags", "tags"),
                ],
            ),
        ],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn demo4_unknown_api_discovered_generated_migrated() {
    let env = TestEnv::new().await;
    // source system: every 7th request is rate limited (HTTP 429 + Retry-After)
    let api_a = ma_example_api::start(ma_example_api::ApiConfig { rate_limit_every: Some(7), ..Default::default() }).await.unwrap();
    // second, empty instance used as a writable destination (test environment)
    let api_b = ma_example_api::start(ma_example_api::ApiConfig { empty: true, ..Default::default() }).await.unwrap();
    env.app.set_credential("fieldops-key", "test-key-123").await.unwrap();
    rest_conn(&env, "fieldops", &api_a.base_url, Environment::Production).await;
    rest_conn(&env, "fieldops-new", &api_b.base_url, Environment::Test).await;

    // no connector exists yet
    let err = env.app.discover("fieldops", Actor::Cli).await.unwrap_err();
    assert!(err.message.contains("no connector yet"), "{}", err.message);

    // 1-5: discover from OpenAPI, generate, validate
    let generated = env
        .app
        .generate_connector(
            &GenerateConnectorRequest {
                source: DocumentSource::Url { url: format!("{}/openapi.json", api_a.base_url), connection: "fieldops".into() },
                connector_id: "fieldops-legacy".into(),
                attach_to_connection: Some("fieldops".into()),
            },
            Actor::Mcp,
        )
        .await
        .unwrap();
    println!("{}", serde_json::to_string_pretty(&generated.spec).unwrap());
    assert!(generated.validation_errors.is_empty());
    assert_eq!(generated.status, ConnectorStatus::Draft);
    assert_eq!(generated.spec.base_path, "/api/v2");
    assert_eq!(generated.spec.authentication, AuthSpec::ApiKeyHeader { header: "X-Api-Key".into() });
    let techs = generated.spec.resource("technicians").expect("technicians resource");
    let wos = generated.spec.resource("work_orders").expect("work_orders resource");
    assert_eq!(techs.identity_field, "tech_id");
    assert_eq!(wos.identity_field, "wo_number");
    match &techs.list.as_ref().unwrap().pagination {
        Pagination::Cursor { cursor_param, next_cursor_path, .. } => {
            assert_eq!(cursor_param, "after");
            assert_eq!(next_cursor_path, "next_cursor");
        }
        other => panic!("expected cursor pagination, got {other:?}"),
    }
    assert_eq!(techs.list.as_ref().unwrap().records_path, "items");
    assert!(techs.create.is_some() && techs.lookup.is_some());
    assert!(wos.relationships.iter().any(|r| r.from_field == "assigned_tech" && r.to_object == "technicians"));
    assert!(!generated.suspicious_text.is_empty(), "prompt-injection text in the docs is detected and reported");
    assert!(!serde_json::to_string(&generated.spec).unwrap().contains("169.254"), "injected instructions had no effect on the spec");
    assert_eq!(generated.manifest.evidence("read"), Evidence::Documented);

    // 6: contract tests against the live system (read-only) → tested; evidence upgraded
    let t = env.app.test_connector("fieldops", Actor::Mcp).await.unwrap();
    for r in &t.results {
        println!("{} {} {}", if r.passed { "pass" } else { "FAIL" }, r.name, r.detail);
    }
    assert!(t.passed);
    assert_eq!(t.status, ConnectorStatus::Tested);
    assert_eq!(t.manifest.evidence("read"), Evidence::Verified);
    assert_eq!(t.manifest.evidence("pagination"), Evidence::Verified);
    assert_eq!(t.manifest.evidence("write"), Evidence::Documented, "writes are not claimed as verified by contract tests");

    let d = env.app.discover("fieldops", Actor::Cli).await.unwrap();
    assert_eq!(d.identity.version.as_deref(), Some("2.4.1"));

    // 7: migrate API → JSONL files (source connector only needs to be tested)
    let out = env.path("fieldops_export");
    env.files_conn("archive", &out, "jsonl", Environment::Production).await;
    let plan = env.plan("fieldops", "archive", api_mapping(), 25).await;
    assert!(plan.approvable, "{:?}", plan.blocking_risks);
    assert_eq!(plan.execution_order, vec!["technicians", "work_orders"]);
    let (_, report, v) = env.execute(&plan.plan_id).await;
    assert_eq!(report.records_written, (api_a.technicians().len() + api_a.work_orders().len()) as u64);
    assert!(v.discrepancies.is_empty(), "{:?}", v.discrepancies);
    assert_eq!(report.final_status, FinalStatus::CompletedAndVerified);
    assert!(api_a.rate_limited_count() > 0, "rate limiting was exercised and handled");

    // 8: write into the new system through the generated connector (registry reuse)
    let gen_b = env
        .app
        .generate_connector(
            &GenerateConnectorRequest {
                source: DocumentSource::Url { url: format!("{}/openapi.json", api_b.base_url), connection: "fieldops-new".into() },
                connector_id: "fieldops-legacy".into(),
                attach_to_connection: Some("fieldops-new".into()),
            },
            Actor::Mcp,
        )
        .await
        .unwrap();
    assert!(gen_b.reused_existing, "identical documents reuse the registered connector");
    // writes need an *active* connector: unapproved generated connectors cannot be used for writes
    let blocked = env.plan("fieldops", "fieldops-new", api_mapping(), 25).await;
    assert!(!blocked.approvable && blocked.blocking_risks.iter().any(|r| r.contains("connector_not_active")));
    env.app.activate_connector(&generated.connector_ref, &OperatorConsent::for_tests("test-operator").unwrap()).await.unwrap();

    let plan2 = env.plan("fieldops", "fieldops-new", api_mapping(), 25).await;
    assert!(plan2.approvable, "{:?}", plan2.blocking_risks);
    let (_, report2, v2) = env.execute(&plan2.plan_id).await;
    assert!(v2.discrepancies.is_empty(), "{:?}", v2.discrepancies);
    assert_eq!(report2.final_status, FinalStatus::CompletedAndVerified);
    assert_eq!(api_b.technicians().len(), api_a.technicians().len());
    assert_eq!(api_b.work_orders().len(), api_a.work_orders().len());
    assert_eq!(api_b.work_orders(), api_a.work_orders(), "records are identical in the new system");

    api_a.shutdown();
    api_b.shutdown();
}
