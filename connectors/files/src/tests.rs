use super::*;

fn conn(dir: &Path, format: &str) -> FileConnector {
    FileConnector::new(&json!({"path": dir, "format": format})).unwrap()
}

async fn read_all(c: &FileConnector, object: &str, batch: usize) -> Vec<Record> {
    let mut out = vec![];
    let mut cursor: Option<Cursor> = None;
    loop {
        let b = c.read_batch(object, cursor.as_ref(), batch).await.unwrap();
        out.extend(b.records);
        match b.next_cursor {
            Some(c) => cursor = Some(c),
            None => return out,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn csv_streaming_types_and_nulls() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("customers.csv"),
        "id,name,amount,joined,code,note\n1,Ann,12.50,2024-01-02,007,\n2,\"Bo, Jr\",3.00,2024-02-03,010,hi\n3,Zoë,,2024-03-04,011,\"multi\nline\"\n",
    )
    .unwrap();
    let c = conn(dir.path(), "csv");
    let schema = c.discover_schema().await.unwrap();
    let o = schema.object("customers").unwrap();
    assert_eq!(o.field("amount").unwrap().field_type, FieldType::Decimal);
    assert_eq!(o.field("code").unwrap().field_type, FieldType::String);
    assert_eq!(o.field("joined").unwrap().field_type, FieldType::Date);
    let all = read_all(&c, "customers", 1).await;
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].fields["amount"], Value::Decimal("12.50".into()));
    assert_eq!(all[0].fields["note"], Value::Null);
    assert_eq!(all[1].fields["name"], Value::String("Bo, Jr".into()));
    assert_eq!(all[2].fields["name"], Value::String("Zoë".into()));
    assert_eq!(all[2].fields["note"], Value::String("multi\nline".into()));
    assert_eq!(all[0].fields["code"], Value::String("007".into()));
    assert_eq!(all[1].id, "2");
}

/// Regression: column types are inferred from the first 1,000 rows. Later rows that the
/// inferred type would alter must arrive exactly as written, not converted.
#[tokio::test(flavor = "multi_thread")]
async fn csv_rows_beyond_the_type_sample_are_not_altered() {
    let dir = tempfile::tempdir().unwrap();
    let mut csv = String::from("id,zip,phone,seen\n");
    for i in 1..=SCHEMA_SAMPLE {
        csv.push_str(&format!("{i},{},{},2024-01-02\n", 10_000 + i, 46_700_000_000u64 + i as u64));
    }
    // rows 1,001 and 1,002: outside the sample the types were inferred from
    csv.push_str("7001,01067,+46701112233,2024-03-04T05:06:07Z\n");
    csv.push_str("7002,00501,0046701112233,2024-03-05\n");
    std::fs::write(dir.path().join("people.csv"), csv).unwrap();
    let c = conn(dir.path(), "csv");
    let schema = c.discover_schema().await.unwrap();
    let o = schema.object("people").unwrap();
    assert_eq!(o.field("zip").unwrap().field_type, FieldType::Int, "the sample only held plain integers");
    assert_eq!(o.field("phone").unwrap().field_type, FieldType::Int);
    assert_eq!(o.field("seen").unwrap().field_type, FieldType::Date);
    let all = read_all(&c, "people", 400).await;
    assert_eq!(all.len(), SCHEMA_SAMPLE + 2);
    assert_eq!(all[0].fields["zip"], Value::Int(10_001));
    let late = &all[SCHEMA_SAMPLE];
    assert_eq!(late.fields["zip"].to_text().as_deref(), Some("01067"), "leading zero kept");
    assert_eq!(late.fields["phone"].to_text().as_deref(), Some("+46701112233"), "plus sign kept");
    match &late.fields["seen"] {
        Value::DateTime(dt) => assert_eq!(dt.to_rfc3339(), "2024-03-04T05:06:07+00:00", "time of day kept"),
        other => panic!("expected a date-time, got {other:?}"),
    }
    let last = &all[SCHEMA_SAMPLE + 1];
    assert_eq!(last.fields["zip"].to_text().as_deref(), Some("00501"));
    assert_eq!(last.fields["phone"].to_text().as_deref(), Some("0046701112233"));
    assert!(matches!(last.fields["seen"], Value::Date(_)));
    assert!(all.iter().all(|r| r.fields.values().all(|v| !v.is_problem())), "none of these values is invalid");
}

#[tokio::test(flavor = "multi_thread")]
async fn json_array_and_jsonl_nested() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("orders.json"),
        r#"[ {"id": 1, "lines": [{"sku": "A", "qty": 2}], "addr": {"city": "Oslo"}, "x": null},
             {"id": 2, "note": "a \"quoted\" ] brace }", "amount": 10.10} ]"#,
    )
    .unwrap();
    let c = conn(dir.path(), "json");
    let all = read_all(&c, "orders", 1).await;
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].get_path("addr.city"), Some(&Value::String("Oslo".into())));
    assert_eq!(all[0].get_path("lines.0.qty"), Some(&Value::Int(2)));
    assert_eq!(all[0].fields["x"], Value::Null);
    assert!(!all[1].fields.contains_key("x"), "missing stays missing");
    assert_eq!(all[1].fields["amount"], Value::Decimal("10.10".into()));

    std::fs::write(dir.path().join("ev.jsonl"), "{\"id\":\"a\"}\n\n{\"id\":\"b\",\"v\":[]}\n").unwrap();
    let c = conn(dir.path(), "jsonl");
    let all = read_all(&c, "ev", 10).await;
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].fields["v"], Value::Array(vec![]));
}

#[tokio::test(flavor = "multi_thread")]
async fn write_resume_truncates_to_token() {
    for format in ["csv", "jsonl", "json"] {
        let dir = tempfile::tempdir().unwrap();
        let c = conn(dir.path(), format);
        let spec = SinkObjectSpec {
            object: "out".into(),
            fields: vec![FieldDef::new("k", FieldType::String), FieldDef::new("v", FieldType::Int)],
            key_field: Some("k".into()),
            mode: WriteMode::Upsert,
        };
        let req = |i: i64| WriteRequest {
            source_id: i.to_string(),
            fields: [("k".to_string(), Value::String(format!("k{i}"))), ("v".to_string(), Value::Int(i))].into_iter().collect(),
        };
        c.prepare(&spec, None).await.unwrap();
        c.write_batch("out", vec![req(1), req(2)]).await.unwrap();
        let tok = c.durable_token("out").await.unwrap();
        // a batch that is written but never checkpointed (simulated crash)
        c.write_batch("out", vec![req(3)]).await.unwrap();
        c.durable_token("out").await.unwrap();
        drop(c);
        // resume
        let c = conn(dir.path(), format);
        c.prepare(&spec, tok.as_ref()).await.unwrap();
        c.write_batch("out", vec![req(3), req(4)]).await.unwrap();
        c.durable_token("out").await.unwrap();
        c.finish("out").await.unwrap();
        let all = read_all(&c, "out", 100).await;
        let keys: Vec<String> = all.iter().map(|r| r.fields["k"].to_text().unwrap()).collect();
        assert_eq!(keys, vec!["k1", "k2", "k3", "k4"], "format {format}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_to_overwrite_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("out.csv"), "a\n1\n").unwrap();
    let c = conn(dir.path(), "csv");
    let spec = SinkObjectSpec { object: "out".into(), fields: vec![], key_field: None, mode: WriteMode::Insert };
    let err = c.prepare(&spec, None).await.unwrap_err();
    assert_eq!(err.class, ErrorClass::Policy);
}

#[test]
fn rejects_path_traversal() {
    let c = FileConnector::new(&json!({"path": ".", "format": "csv"})).unwrap();
    assert!(c.file_for("../secrets").is_err());
    assert!(c.file_for("a/b").is_err());
}
