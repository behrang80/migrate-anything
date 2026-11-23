//! Demo 2 — Financial migration: customers, invoices, invoice line items and currencies from a
//! JSON export into a new ledger (JSON Lines). Exact decimal handling (amounts → minor units),
//! localized dates, relationship translation, explicit exception reporting for bad records, and
//! historical audit metadata preserved in the exception archive — never fabricated or backdated.

mod common;

use common::*;
use ma_core::Actor;
use ma_types::*;
use serde_json::json;

const BAD_CURRENCY_INVOICES: [u32; 3] = [7, 19, 42];

fn write_finance_json(dir: &std::path::Path) -> (usize, usize, usize, usize) {
    let mut r = Rng(11);
    let currencies = json!([
        {"code": "SEK", "name": "Swedish krona", "minor_units": 2},
        {"code": "EUR", "name": "Euro", "minor_units": 2},
        {"code": "JPY", "name": "Yen", "minor_units": 0}
    ]);
    std::fs::write(dir.join("currencies.json"), serde_json::to_string_pretty(&currencies).unwrap()).unwrap();
    let customers: Vec<_> = (1..=40)
        .map(|i| json!({"customer_no": format!("K{i:04}"), "legal_name": format!("Kund {i} AB"), "vat_no": format!("SE55{i:08}01"), "payment_terms_days": ([10, 30, 45][i % 3])}))
        .collect();
    std::fs::write(dir.join("customers.json"), serde_json::to_string_pretty(&customers).unwrap()).unwrap();
    let mut invoices = String::from("[\n");
    let mut lines = String::from("[\n");
    let mut line_id = 0;
    let n_invoices = 120;
    for i in 1..=n_invoices {
        let ccy = if BAD_CURRENCY_INVOICES.contains(&i) { "XXX" } else { r.pick(&["SEK", "EUR"]) };
        let nlines = 1 + r.below(4) as usize;
        let mut total_cents: u64 = 0;
        for _ in 0..nlines {
            line_id += 1;
            let qty = 1 + r.below(10);
            let cents = 100 + r.below(500_000);
            total_cents += qty * cents;
            if line_id > 1 {
                lines.push_str(",\n");
            }
            // amounts as exact JSON decimals with trailing zeros, e.g. 1234.50
            lines.push_str(&format!(
                r#"{{"line_id": {line_id}, "invoice_id": {i}, "description": "Item {line_id}", "quantity": {qty}, "unit_price": {}.{:02}, "vat_rate": 0.25}}"#,
                cents / 100,
                cents % 100
            ));
        }
        if i > 1 {
            invoices.push_str(",\n");
        }
        invoices.push_str(&format!(
            r#"{{"invoice_id": {i}, "invoice_no": "INV-2024-{i:05}", "customer_no": "K{:04}", "currency": "{ccy}", "issue_date": "{:02}.{:02}.2024", "total": {}.{:02}, "status": "{}", "posted_at": "2024-06-01T09:00:00+02:00", "audit_trail": [{{"at": "2024-05-30T12:00:00Z", "by": "clerk-{}", "event": "created"}}, {{"at": "2024-06-01T07:00:00Z", "by": "controller", "event": "posted"}}]}}"#,
            1 + r.below(40),
            1 + r.below(28),
            1 + r.below(12),
            total_cents / 100,
            total_cents % 100,
            r.pick(&["paid", "open", "overdue"]),
            r.below(5)
        ));
    }
    invoices.push_str("\n]\n");
    lines.push_str("\n]\n");
    std::fs::write(dir.join("invoices.json"), invoices).unwrap();
    std::fs::write(dir.join("invoice_lines.json"), lines).unwrap();
    (3, 40, n_invoices as usize, line_id)
}

fn finance_mapping() -> MappingSpec {
    let mut inv = object(
        "invoices",
        "bills",
        Some("legacy_invoice_id"),
        vec![
            fx("legacy_invoice_id", cast("invoice_id", FieldType::String)),
            f("number", "invoice_no"),
            reff("client_ref", "customer_no", "customers"),
            fx("currency_code", enum_map("currency", &[("SEK", "SEK"), ("EUR", "EUR"), ("JPY", "JPY")])),
            fx("issued_on", Expr::ParseDate { input: Box::new(Expr::field("issue_date")), format: "%d.%m.%Y".into(), datetime: false }),
            fx("total_minor", Expr::ScaleDecimal { input: Box::new(Expr::field("total")), pow: 2 }),
            fx("state", enum_map("status", &[("paid", "settled"), ("open", "outstanding"), ("overdue", "outstanding_overdue")])),
            f("original_posted_at", "posted_at"),
        ],
    );
    // audit_trail has no destination equivalent: it is archived, not re-created
    inv.unmapped_fields = UnmappedFieldPolicy::Archive;
    MappingSpec {
        mapping_version: 1,
        objects: vec![
            object("currencies", "currencies", Some("code"), vec![f("code", "code"), f("name", "name"), f("minor_units", "minor_units")]),
            object(
                "customers",
                "clients",
                Some("legacy_no"),
                vec![
                    f("legacy_no", "customer_no"),
                    f("name", "legal_name"),
                    f("vat_number", "vat_no"),
                    f("terms_days", "payment_terms_days"),
                ],
            ),
            inv,
            object(
                "invoice_lines",
                "bill_lines",
                Some("legacy_line_id"),
                vec![
                    fx("legacy_line_id", cast("line_id", FieldType::String)),
                    reff("bill_ref", "invoice_id", "invoices"),
                    f("text", "description"),
                    f("qty", "quantity"),
                    fx("unit_price_minor", Expr::ScaleDecimal { input: Box::new(Expr::field("unit_price")), pow: 2 }),
                    f("vat_rate", "vat_rate"),
                ],
            ),
        ],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn demo2_finance_json_to_ledger() {
    let env = TestEnv::new().await;
    let src = env.path("erp_export");
    let out = env.path("ledger");
    let (ncur, ncust, ninv, nlines) = write_finance_json(&src);
    env.app
        .add_connection(
            ConnectionConfig {
                name: "old-erp".into(),
                kind: "files".into(),
                environment: Environment::Production,
                settings: json!({"path": src, "format": "json",
                    "id_fields": {"currencies": "code", "customers": "customer_no", "invoices": "invoice_id", "invoice_lines": "line_id"}}),
                credential_ref: None,
                allowed_hosts: vec![],
                connector_ref: None,
            },
            Actor::Cli,
        )
        .await
        .unwrap();
    env.files_conn("new-ledger", &out, "jsonl", Environment::Production).await;

    let plan = env.plan("old-erp", "new-ledger", finance_mapping(), 50).await;
    assert!(plan.approvable, "{:?}", plan.blocking_risks);
    let pos = |o: &str| plan.execution_order.iter().position(|x| x == o).unwrap();
    assert!(pos("customers") < pos("invoices") && pos("invoices") < pos("invoice_lines"));
    assert!(plan.markdown.contains("audit_trail"), "plan discloses that audit history is archived");

    // dry run already surfaces the bad currency
    let dry = env.app.dry_run(&plan.plan_id, 200).await.unwrap();
    assert!(!dry.wrote_to_destination);
    let inv = dry.objects.iter().find(|o| o.source_object == "invoices").unwrap();
    assert_eq!(inv.transform_failed, BAD_CURRENCY_INVOICES.len() as u64);
    assert!(!out.join("bills.jsonl").exists(), "dry run must not create destination files");

    let (job, report, v) = env.execute(&plan.plan_id).await;

    // exact, evidence-based figures
    let bad_lines = {
        let lines: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(src.join("invoice_lines.json")).unwrap()).unwrap();
        lines.as_array().unwrap().iter().filter(|l| BAD_CURRENCY_INVOICES.contains(&(l["invoice_id"].as_u64().unwrap() as u32))).count()
    };
    let total = ncur + ncust + ninv + nlines;
    assert_eq!(report.records_discovered, total as u64);
    assert_eq!(
        report.records_failed,
        (BAD_CURRENCY_INVOICES.len() + bad_lines) as u64,
        "bad invoices and their lines fail; nothing silently dropped"
    );
    assert_eq!(report.records_written, (total - BAD_CURRENCY_INVOICES.len() - bad_lines) as u64);
    assert_eq!(report.records_verified, report.records_written);
    assert!(v.discrepancies.is_empty(), "{:?}", v.discrepancies);
    assert_eq!(report.final_status, FinalStatus::CompletedWithExceptions);
    assert!(report.unsupported_fields.iter().any(|f| f.starts_with("invoices.audit_trail")));

    // archived history is preserved verbatim, and never written as destination history
    let exc = env.app.exceptions(&job, 10_000).await.unwrap();
    let archived: Vec<_> = exc.iter().filter(|e| e.kind == "archived_field" && e.field.as_deref() == Some("audit_trail")).collect();
    assert_eq!(archived.len(), ninv - BAD_CURRENCY_INVOICES.len());
    assert!(archived[0].value.as_ref().unwrap().as_str().unwrap().contains("controller"));
    let not_written: Vec<_> = exc.iter().filter(|e| e.kind == "record_not_written").collect();
    assert_eq!(not_written.len(), BAD_CURRENCY_INVOICES.len() + bad_lines, "failed source records are preserved in full");
    let bills = std::fs::read_to_string(out.join("bills.jsonl")).unwrap();
    assert!(!bills.contains("audit_trail") && !bills.contains("clerk-"));

    // exact decimal arithmetic: total 1234.50 → 123450 minor units; no float artefacts
    let first: serde_json::Value = serde_json::from_str(bills.lines().next().unwrap()).unwrap();
    assert!(first["total_minor"].is_u64() || first["total_minor"].is_i64(), "{first}");
    let lines_out = std::fs::read_to_string(out.join("bill_lines.jsonl")).unwrap();
    assert!(!lines_out.contains("e+") && !lines_out.contains(".0000000"));
    assert!(lines_out.contains("\"vat_rate\":0.25"));
}
