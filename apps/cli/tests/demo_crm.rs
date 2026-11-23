//! Demo 1 — CRM-like migration: CSV export of a legacy CRM → PostgreSQL CRM with a different
//! schema. Organizations, contacts (with a self-referencing manager), opportunities, activities,
//! custom fields, enum conversion and foreign keys enforced by the destination.

mod common;

use std::fmt::Write as _;

use common::*;
use ma_core::Actor;
use ma_types::*;

const DEST_SCHEMA: &str = r#"
CREATE TABLE accounts (
  id serial PRIMARY KEY, external_id text NOT NULL UNIQUE, company_name text NOT NULL, sector text,
  homepage text, revenue numeric(14,2), tier text, source_created_on date);
CREATE TABLE people (
  id serial PRIMARY KEY, legacy_id text NOT NULL UNIQUE, account_id int NOT NULL REFERENCES accounts(id),
  given_name text NOT NULL, family_name text NOT NULL, email text, phone_number text, linkedin_url text,
  reports_to int REFERENCES people(id));
CREATE TABLE deals (
  id serial PRIMARY KEY, legacy_ref text NOT NULL UNIQUE, account_id int NOT NULL REFERENCES accounts(id),
  primary_contact_id int REFERENCES people(id), deal_name text NOT NULL, value numeric(14,2),
  currency_code char(3), stage text NOT NULL CHECK (stage IN ('qualification','negotiation','closed_won','closed_lost')),
  expected_close date);
CREATE TABLE interactions (
  id serial PRIMARY KEY, legacy_ref text NOT NULL UNIQUE, person_id int NOT NULL REFERENCES people(id),
  deal_id int REFERENCES deals(id), kind text NOT NULL, subject text, occurred_at timestamptz NOT NULL, details text);
"#;

pub fn write_crm_csv(dir: &std::path::Path, orgs: usize, contacts: usize, opps: usize, acts: usize) {
    let mut r = Rng(7);
    let industries = ["Manufacturing", "Retail", "Software", "Healthcare", "Logistics"];
    let tiers = ["gold", "silver", "bronze", ""];
    let first = ["Ann", "Bo", "Zoë", "Søren", "Mía", "Li", "Omar", "Ines"];
    let last = ["Lee", "Berg", "Núñez", "O'Brien", "Smith, Jr.", "Kowalski"];
    let stages = ["prospect", "negotiation", "won", "lost"];
    let mut s = String::from("org_id,org_name,industry,website,annual_revenue,cf_tier,created_on\n");
    for i in 1..=orgs {
        let _ = writeln!(
            s,
            "{i},\"Org {i} {}\",{},https://org{i}.example,{}.{:02},{},2019-{:02}-{:02}",
            r.pick(&["AB", "Ltd", "GmbH, Inc"]),
            r.pick(&industries),
            r.below(9_000_000) + 1000,
            r.below(100),
            r.pick(&tiers),
            r.below(12) + 1,
            r.below(28) + 1
        );
    }
    std::fs::write(dir.join("organizations.csv"), s).unwrap();
    let mut s = String::from("contact_id,org_id,first_name,last_name,email,phone,cf_linkedin,manager_id\n");
    for i in 1..=contacts {
        let mgr = if i > 5 && r.below(10) < 3 { (r.below(i as u64 - 1) + 1).to_string() } else { String::new() };
        let email = if r.below(10) == 0 { String::new() } else { format!("c{i}@example.com") };
        let li = if r.below(2) == 0 { format!("https://linkedin.example/in/c{i}") } else { String::new() };
        let _ = writeln!(
            s,
            "C{i:05},{},{},\"{}\",{email},+46 70 {:03} {:02} {:02},{li},{}",
            r.below(orgs as u64) + 1,
            r.pick(&first),
            r.pick(&last).replace('"', "\"\""),
            r.below(1000),
            r.below(100),
            r.below(100),
            if mgr.is_empty() { String::new() } else { format!("C{:05}", mgr.parse::<usize>().unwrap()) }
        );
    }
    std::fs::write(dir.join("contacts.csv"), s).unwrap();
    let mut s = String::from("opp_id,org_id,contact_id,title,amount,currency,stage,close_date\n");
    for i in 1..=opps {
        let _ = writeln!(
            s,
            "{i},{},C{:05},Deal {i},{}.{:02},{},{},2025-{:02}-{:02}",
            r.below(orgs as u64) + 1,
            r.below(contacts as u64) + 1,
            r.below(500_000),
            r.below(100),
            r.pick(&["sek", "eur", "usd"]),
            r.pick(&stages),
            r.below(12) + 1,
            r.below(28) + 1
        );
    }
    std::fs::write(dir.join("opportunities.csv"), s).unwrap();
    let mut s = String::from("act_id,contact_id,opp_id,kind,subject,occurred_at,notes\n");
    for i in 1..=acts {
        let opp = if r.below(3) == 0 { String::new() } else { (r.below(opps as u64) + 1).to_string() };
        let _ = writeln!(
            s,
            "A{i},C{:05},{opp},{},\"Subject {i}\",2024-{:02}-{:02}T{:02}:15:00Z,\"Note line 1\nline 2 for {i}\"",
            r.below(contacts as u64) + 1,
            r.pick(&["call", "email", "meeting"]),
            r.below(12) + 1,
            r.below(28) + 1,
            r.below(24)
        );
    }
    std::fs::write(dir.join("activities.csv"), s).unwrap();
}

fn crm_mapping() -> MappingSpec {
    MappingSpec {
        mapping_version: 1,
        objects: vec![
            object(
                "organizations",
                "accounts",
                Some("external_id"),
                vec![
                    fx("external_id", cast("org_id", FieldType::String)),
                    f("company_name", "org_name"),
                    f("sector", "industry"),
                    f("homepage", "website"),
                    f("revenue", "annual_revenue"),
                    f("tier", "cf_tier"),
                    f("source_created_on", "created_on"),
                ],
            ),
            object(
                "contacts",
                "people",
                Some("legacy_id"),
                vec![
                    f("legacy_id", "contact_id"),
                    reff("account_id", "org_id", "organizations"),
                    f("given_name", "first_name"),
                    f("family_name", "last_name"),
                    f("email", "email"),
                    f("phone_number", "phone"),
                    f("linkedin_url", "cf_linkedin"),
                    reff("reports_to", "manager_id", "contacts"),
                ],
            ),
            object(
                "opportunities",
                "deals",
                Some("legacy_ref"),
                vec![
                    fx("legacy_ref", cast("opp_id", FieldType::String)),
                    reff("account_id", "org_id", "organizations"),
                    reff("primary_contact_id", "contact_id", "contacts"),
                    f("deal_name", "title"),
                    f("value", "amount"),
                    fx("currency_code", Expr::Upper { input: Box::new(Expr::field("currency")) }),
                    fx(
                        "stage",
                        enum_map(
                            "stage",
                            &[
                                ("prospect", "qualification"),
                                ("negotiation", "negotiation"),
                                ("won", "closed_won"),
                                ("lost", "closed_lost"),
                            ],
                        ),
                    ),
                    f("expected_close", "close_date"),
                ],
            ),
            object(
                "activities",
                "interactions",
                Some("legacy_ref"),
                vec![
                    f("legacy_ref", "act_id"),
                    reff("person_id", "contact_id", "contacts"),
                    reff("deal_id", "opp_id", "opportunities"),
                    f("kind", "kind"),
                    f("subject", "subject"),
                    f("occurred_at", "occurred_at"),
                    f("details", "notes"),
                ],
            ),
        ],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn demo1_crm_csv_to_postgres() {
    let Some(admin) = pg_admin_url() else { return };
    let env = TestEnv::new().await;
    let src = env.path("crm_export");
    write_crm_csv(&src, 200, 1000, 600, 2000);
    let db = TempDb::create(&admin, "ma_crm").await;
    db.exec(DEST_SCHEMA).await;

    env.app
        .add_connection(
            ConnectionConfig {
                name: "legacy-crm".into(),
                kind: "files".into(),
                environment: Environment::Production,
                settings: serde_json::json!({"path": src, "format": "csv",
                    "id_fields": {"organizations": "org_id", "contacts": "contact_id", "opportunities": "opp_id", "activities": "act_id"}}),
                credential_ref: None,
                allowed_hosts: vec![],
                connector_ref: None,
            },
            Actor::Cli,
        )
        .await
        .unwrap();
    env.pg_conn("new-crm", &db.url, Environment::Production).await;

    // discovery: relationships are found (FK constraints on the destination, inference on CSV)
    let s = env.app.discover("legacy-crm", Actor::Cli).await.unwrap();
    assert_eq!(s.schema.objects.len(), 4);
    assert!(
        s.schema.relationships.iter().any(|r| r.from_object == "contacts" && r.from_field == "org_id" && r.to_object == "organizations")
    );
    let d = env.app.discover("new-crm", Actor::Cli).await.unwrap();
    assert!(d.schema.relationships.iter().any(|r| r.from_object == "people" && r.from_field == "reports_to" && r.to_object == "people"));

    // AI-assisted suggestion pairs the objects despite different names
    let sugg = env.app.suggest_mapping("legacy-crm", "new-crm", &[]).await.unwrap();
    let pair = |s: &str| sugg.pairings.iter().find(|p| p.source_object == s).map(|p| p.target_object.clone());
    assert_eq!(pair("organizations").as_deref(), Some("accounts"));
    assert_eq!(pair("contacts").as_deref(), Some("people"));
    assert_eq!(pair("opportunities").as_deref(), Some("deals"));
    assert_eq!(pair("activities").as_deref(), Some("interactions"));
    let people = sugg.mapping.object("contacts").unwrap();
    assert!(
        people.fields.iter().any(|f| f.target == "account_id" && matches!(f.expr, Expr::Ref { .. })),
        "FK pairs become id-map references"
    );

    // reviewed mapping → plan
    let plan = env.plan("legacy-crm", "new-crm", crm_mapping(), 250).await;
    println!("{}", plan.markdown);
    assert!(plan.approvable, "blocking: {:?}", plan.blocking_risks);
    assert_eq!(plan.execution_order, vec!["organizations", "contacts", "opportunities", "activities"]);
    assert!(plan.markdown.contains("Second pass"), "self reference is deferred");

    // dry run writes nothing
    let preview = env.app.preview_plan(&plan.plan_id, true, 100).await.unwrap();
    assert!(!preview.dry_run.as_ref().unwrap().wrote_to_destination);
    assert_eq!(db.scalar_i64("SELECT count(*) FROM accounts").await, 0);

    let (_, report, v) = env.execute(&plan.plan_id).await;
    assert_eq!(report.records_written, 3800);
    assert_eq!(report.records_failed, 0);
    assert!(v.discrepancies.is_empty(), "{:?}", v.discrepancies);
    assert!(report.relationships_checked > 3000);
    assert_eq!(report.relationships_checked, report.relationships_preserved);
    assert_eq!(report.final_status, FinalStatus::CompletedAndVerified);

    // destination facts
    assert_eq!(db.scalar_i64("SELECT count(*) FROM people").await, 1000);
    let src_mgr = std::fs::read_to_string(src.join("contacts.csv")).unwrap().lines().skip(1).filter(|l| !l.ends_with(',')).count() as i64;
    assert_eq!(
        db.scalar_i64("SELECT count(*) FROM people WHERE reports_to IS NOT NULL").await,
        src_mgr,
        "second pass wrote every manager reference"
    );
    assert!(db.scalar_i64("SELECT count(*) FROM deals WHERE stage = 'closed_won'").await > 0);
    assert_eq!(
        db.scalar_i64("SELECT count(*) FROM interactions i JOIN people p ON p.id = i.person_id").await,
        2000,
        "every interaction points at a migrated person"
    );
    db.drop_db().await;
}
