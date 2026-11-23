//! Integration tests against a live PostgreSQL server.
//!
//! Set `MA_TEST_PG_URL` to an admin URL (a role allowed to create databases), e.g.
//! `postgres://ma_test@127.0.0.1:55432/postgres`. Every test creates and drops its own database.

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use ma_connector_postgres::{PostgresConnector, handle, quote_ident};
use ma_connector_sdk::{ConnectorHandle, SinkObjectSpec, WriteRequest};
use ma_types::{ErrorClass, Evidence, FieldDef, FieldType, Value, WriteMode};
use sqlx::AssertSqlSafe;
use sqlx::postgres::PgPoolOptions;

const SCHEMA_SQL: &str = r#"
CREATE TABLE customers (
    id bigserial PRIMARY KEY,
    email text NOT NULL UNIQUE,
    balance numeric(12,2),
    created_at timestamptz NOT NULL DEFAULT now(),
    meta jsonb,
    nickname varchar(40)
);
CREATE TABLE orders (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    customer_id bigint NOT NULL REFERENCES customers(id),
    code text NOT NULL,
    total numeric(10,2) NOT NULL,
    tags text[]
);
CREATE UNIQUE INDEX orders_code_idx ON orders (code);
CREATE TABLE people (
    id bigserial PRIMARY KEY,
    ext_id text NOT NULL UNIQUE,
    name text NOT NULL,
    amount numeric(10,2),
    tags text[],
    attrs jsonb
);
"#;

fn admin_url() -> Option<String> {
    match std::env::var("MA_TEST_PG_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            eprintln!("skipped: MA_TEST_PG_URL not set");
            None
        }
    }
}

/// Create a fresh database with the test schema, run `body`, and always drop the database.
async fn with_db<F, Fut>(body: F)
where
    F: FnOnce(ConnectorHandle, String) -> Fut,
    Fut: Future<Output = ()>,
{
    let Some(admin) = admin_url() else { return };
    let db = format!("ma_t_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let admin_pool = PgPoolOptions::new().max_connections(1).connect(&admin).await.expect("admin connect");
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {}", quote_ident(&db)))).execute(&admin_pool).await.expect("create database");

    let mut url = url::Url::parse(&admin).expect("admin url");
    url.set_path(&format!("/{db}"));
    let url = url.to_string();

    let result = AssertUnwindSafe(async {
        let setup = PgPoolOptions::new().max_connections(1).connect(&url).await.expect("connect test db");
        sqlx::raw_sql(SCHEMA_SQL).execute(&setup).await.expect("create schema");
        setup.close().await;
        let conn = PostgresConnector::connect(&serde_json::json!({ "url": url }), None).await.expect("connector");
        body(handle(conn), url.clone()).await;
    })
    .catch_unwind()
    .await;

    sqlx::query(AssertSqlSafe(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quote_ident(&db))))
        .execute(&admin_pool)
        .await
        .expect("drop database");
    admin_pool.close().await;
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}

async fn exec(url: &str, sql: &'static str) {
    let pool = PgPoolOptions::new().max_connections(1).connect(url).await.unwrap();
    sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    pool.close().await;
}

fn req(id: &str, fields: &[(&str, Value)]) -> WriteRequest {
    WriteRequest { source_id: id.to_string(), fields: fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect() }
}

fn people_spec(mode: WriteMode) -> SinkObjectSpec {
    SinkObjectSpec {
        object: "people".into(),
        fields: vec![
            FieldDef::new("ext_id", FieldType::String),
            FieldDef::new("name", FieldType::String),
            FieldDef::new("amount", FieldType::Decimal),
            FieldDef::new("tags", FieldType::Array),
            FieldDef::new("attrs", FieldType::Object),
        ],
        key_field: Some("ext_id".into()),
        mode,
    }
}

#[tokio::test]
async fn connect_rejects_password_in_url() {
    let err = PostgresConnector::connect(&serde_json::json!({ "url": "postgres://u:secret@127.0.0.1:1/db" }), None).await.unwrap_err();
    assert_eq!(err.class, ErrorClass::Config);
    assert!(!err.message.contains("secret"));
}

#[tokio::test]
async fn discovers_schema() {
    with_db(|h, _url| async move {
        let id = h.connector.identify();
        assert_eq!(id.kind, "postgres");
        assert!(id.name.starts_with("ma_t_"));
        assert!(id.version.is_some());
        assert!(id.locator.ends_with("#public"));
        let caps = h.connector.capabilities();
        assert_eq!(caps.evidence("upsert"), Evidence::Verified);
        assert_eq!(caps.evidence("attachments"), Evidence::Unsupported);
        assert!(h.connector.connector_version().starts_with("builtin-"));
        h.connector.check_connection().await.unwrap();

        let schema = h.connector.discover_schema().await.unwrap();
        let customers = schema.object("customers").expect("customers");
        assert_eq!(customers.primary_key, vec!["id".to_string()]);
        assert!(customers.unique_keys.contains(&vec!["email".to_string()]));
        assert_eq!(customers.estimated_count, Some(0));
        let f = |n: &str| customers.field(n).unwrap().clone();
        assert_eq!(f("id").field_type, FieldType::Int);
        assert_eq!(f("id").native_type.as_deref(), Some("int8"));
        assert!(!f("id").required && !f("id").read_only);
        assert_eq!(f("balance").field_type, FieldType::Decimal);
        assert_eq!(f("balance").native_type.as_deref(), Some("numeric(12,2)"));
        assert!(f("balance").nullable);
        assert_eq!(f("created_at").field_type, FieldType::DateTime);
        assert_eq!(f("created_at").native_type.as_deref(), Some("timestamptz"));
        assert!(!f("created_at").nullable && !f("created_at").required);
        assert_eq!(f("meta").field_type, FieldType::Object);
        assert_eq!(f("meta").native_type.as_deref(), Some("jsonb"));
        assert_eq!(f("nickname").native_type.as_deref(), Some("varchar(40)"));
        assert!(f("email").required && !f("email").nullable);

        let orders = schema.object("orders").expect("orders");
        assert!(orders.field("id").unwrap().read_only);
        assert!(!orders.field("id").unwrap().required);
        assert!(orders.unique_keys.contains(&vec!["code".to_string()]));
        assert_eq!(orders.field("tags").unwrap().field_type, FieldType::Array);
        assert_eq!(orders.field("tags").unwrap().native_type.as_deref(), Some("text[]"));
        let rel =
            schema.relationships.iter().find(|r| r.from_object == "orders" && r.from_field == "customer_id").expect("fk relationship");
        assert_eq!(rel.to_object, "customers");
        assert_eq!(rel.to_field, "id");
        assert_eq!(rel.evidence, Evidence::Documented);
    })
    .await;
}

#[tokio::test]
async fn keyset_reads_exact_values() {
    with_db(|h, url| async move {
        exec(
            &url,
            r#"INSERT INTO customers (email, balance, created_at, meta, nickname)
               SELECT 'c' || g || '@x.test', CASE WHEN g = 1 THEN 12.50 ELSE g END,
                      TIMESTAMPTZ '2024-01-02 03:04:05+00' + g * INTERVAL '1 day',
                      CASE WHEN g = 1 THEN '{"a": [1, 2.50]}'::jsonb END,
                      CASE WHEN g = 2 THEN 'nick' END
               FROM generate_series(1, 7) g"#,
        )
        .await;
        let src = h.source.clone().unwrap();
        assert_eq!(src.count("customers").await.unwrap(), Some(7));

        let mut cursor = None;
        let mut pages = 0;
        let mut all = Vec::new();
        loop {
            let batch = src.read_batch("customers", cursor.as_ref(), 3).await.unwrap();
            pages += 1;
            all.extend(batch.records);
            match batch.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
            assert!(pages < 10, "pagination did not terminate");
        }
        assert_eq!(pages, 3);
        assert_eq!(all.len(), 7);
        let ids: Vec<&str> = all.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["1", "2", "3", "4", "5", "6", "7"]);

        let first = &all[0];
        assert_eq!(first.fields["id"], Value::Int(1));
        assert_eq!(first.fields["balance"], Value::Decimal("12.50".into()));
        assert_eq!(first.fields["nickname"], Value::Null);
        assert_eq!(first.fields["email"], Value::String("c1@x.test".into()));
        match &first.fields["created_at"] {
            Value::DateTime(dt) => assert_eq!(dt.timestamp(), 1_704_164_645 + 86_400),
            other => panic!("unexpected {other:?}"),
        }
        let meta = match &first.fields["meta"] {
            Value::Object(m) => m.clone(),
            other => panic!("unexpected {other:?}"),
        };
        // Nested JSON numbers go through `Value::parse_as` (serde_json without arbitrary
        // precision), so the scale of `2.50` inside JSON is not preserved by the shared types.
        match &meta["a"] {
            Value::Array(items) => {
                assert_eq!(items[0], Value::Int(1));
                assert!(matches!(&items[1], Value::Decimal(d) if d.starts_with("2.5")));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(all[1].fields["nickname"], Value::String("nick".into()));
        assert_eq!(all[1].fields["meta"], Value::Null);
        assert_eq!(all[1].fields["balance"], Value::Decimal("2.00".into()));
        assert!(first.provenance.extracted_at.is_some());
        assert!(first.provenance.system.starts_with("ma_t_"));
        assert!(first.provenance.source_created_at.is_some());
    })
    .await;
}

#[tokio::test]
async fn upsert_is_idempotent() {
    with_db(|h, _url| async move {
        let sink = h.sink.clone().unwrap();
        assert!(sink.idempotent_rewrites(WriteMode::Upsert, true));
        assert!(!sink.idempotent_rewrites(WriteMode::Insert, true));
        sink.prepare(&people_spec(WriteMode::Upsert), None).await.unwrap();
        let batch = |suffix: &str| {
            vec![
                req(
                    "s1",
                    &[
                        ("ext_id", Value::String("e1".into())),
                        ("name", Value::String(format!("Ann{suffix}"))),
                        ("amount", Value::Decimal("10.50".into())),
                        ("tags", Value::Array(vec![Value::String("a\"q".into()), Value::Null])),
                        ("attrs", Value::Object(BTreeMap::from([("k".to_string(), Value::Int(1))]))),
                    ],
                ),
                req("s2", &[("ext_id", Value::String("e2".into())), ("name", Value::String(format!("Bob{suffix}")))]),
                req(
                    "s3",
                    &[("ext_id", Value::String("e3".into())), ("name", Value::String(format!("Cy{suffix}"))), ("amount", Value::Null)],
                ),
            ]
        };
        let first: Vec<String> = sink.write_batch("people", batch("")).await.unwrap().into_iter().map(|r| r.unwrap()).collect();
        let second: Vec<String> = sink.write_batch("people", batch(" v2")).await.unwrap().into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(first, second);
        assert_eq!(first.len(), 3);
        assert_eq!(h.source.as_ref().unwrap().count("people").await.unwrap(), Some(3));
        sink.durable_token("people").await.unwrap();
        sink.finish("people").await.unwrap();

        let lookup = h.lookup.clone().unwrap();
        let ann = lookup.lookup_by_key("people", "ext_id", "e1").await.unwrap().expect("e1");
        assert_eq!(ann.id, first[0]);
        assert_eq!(ann.fields["name"], Value::String("Ann v2".into()));
        assert_eq!(ann.fields["amount"], Value::Decimal("10.50".into()));
        assert_eq!(ann.fields["tags"], Value::Array(vec![Value::String("a\"q".into()), Value::Null]));
        assert_eq!(ann.fields["attrs"], Value::Object(BTreeMap::from([("k".to_string(), Value::Int(1))])));

        // Duplicate keys inside one batch: last one wins, no error.
        let res = sink
            .write_batch(
                "people",
                vec![
                    req("x", &[("ext_id", Value::String("e4".into())), ("name", Value::String("D1".into()))]),
                    req("y", &[("ext_id", Value::String("e4".into())), ("name", Value::String("D2".into()))]),
                ],
            )
            .await
            .unwrap();
        assert_eq!(res[0].as_ref().unwrap(), res[1].as_ref().unwrap());
        let d = lookup.lookup_by_key("people", "ext_id", "e4").await.unwrap().unwrap();
        assert_eq!(d.fields["name"], Value::String("D2".into()));
    })
    .await;
}

#[tokio::test]
async fn bad_row_fails_alone() {
    with_db(|h, _url| async move {
        let sink = h.sink.clone().unwrap();
        sink.prepare(&people_spec(WriteMode::Upsert), None).await.unwrap();
        let res = sink
            .write_batch(
                "people",
                vec![
                    req("s1", &[("ext_id", Value::String("g1".into())), ("name", Value::String("ok1".into()))]),
                    req("s2", &[("ext_id", Value::String("g2".into())), ("name", Value::Null)]),
                    req("s3", &[("ext_id", Value::String("g3".into())), ("name", Value::String("ok3".into()))]),
                ],
            )
            .await
            .unwrap();
        assert!(res[0].is_ok(), "{:?}", res[0]);
        let err = res[1].as_ref().unwrap_err();
        assert_eq!(err.class, ErrorClass::Validation, "{err:?}");
        assert!(res[2].is_ok(), "{:?}", res[2]);
        assert_eq!(h.source.as_ref().unwrap().count("people").await.unwrap(), Some(2));
        let lookup = h.lookup.clone().unwrap();
        assert!(lookup.lookup_by_key("people", "ext_id", "g2").await.unwrap().is_none());
        assert!(lookup.lookup_by_key("people", "ext_id", "g3").await.unwrap().is_some());

        // Insert mode: a bad cast fails only its row.
        sink.prepare(&people_spec(WriteMode::Insert), None).await.unwrap();
        let res = sink
            .write_batch(
                "people",
                vec![
                    req("s4", &[("ext_id", Value::String("g4".into())), ("name", Value::String("ok4".into()))]),
                    req(
                        "s5",
                        &[
                            ("ext_id", Value::String("g5".into())),
                            ("name", Value::String("bad".into())),
                            ("amount", Value::String("not-a-number".into())),
                        ],
                    ),
                ],
            )
            .await
            .unwrap();
        assert!(res[0].is_ok());
        assert_eq!(res[1].as_ref().unwrap_err().class, ErrorClass::Validation);
        assert_eq!(h.source.as_ref().unwrap().count("people").await.unwrap(), Some(3));
    })
    .await;
}

#[tokio::test]
async fn prepare_validates_spec() {
    with_db(|h, _url| async move {
        let sink = h.sink.clone().unwrap();
        let mut spec = people_spec(WriteMode::Upsert);
        spec.key_field = Some("name".into());
        assert_eq!(sink.prepare(&spec, None).await.unwrap_err().class, ErrorClass::Config);
        spec.key_field = None;
        assert_eq!(sink.prepare(&spec, None).await.unwrap_err().class, ErrorClass::Config);
        let mut spec = people_spec(WriteMode::Upsert);
        spec.fields.push(FieldDef::new("nope", FieldType::String));
        assert_eq!(sink.prepare(&spec, None).await.unwrap_err().class, ErrorClass::Config);
        let mut spec = people_spec(WriteMode::Upsert);
        spec.object = "missing".into();
        assert_eq!(sink.prepare(&spec, None).await.unwrap_err().class, ErrorClass::Config);
    })
    .await;
}

#[tokio::test]
async fn update_fields_and_lookup() {
    with_db(|h, _url| async move {
        let sink = h.sink.clone().unwrap();
        assert!(sink.supports_updates());
        sink.prepare(&people_spec(WriteMode::Upsert), None).await.unwrap();
        let res = sink
            .write_batch("people", vec![req("s1", &[("ext_id", Value::String("u1".into())), ("name", Value::String("Old".into()))])])
            .await
            .unwrap();
        let dest = res[0].clone().unwrap();

        sink.update_fields(
            "people",
            &dest,
            BTreeMap::from([("name".to_string(), Value::String("New".into())), ("amount".to_string(), Value::Decimal("99.90".into()))]),
        )
        .await
        .unwrap();

        let lookup = h.lookup.clone().unwrap();
        let rec = lookup.lookup_by_key("people", "ext_id", "u1").await.unwrap().expect("found");
        assert_eq!(rec.id, dest);
        assert_eq!(rec.fields["name"], Value::String("New".into()));
        assert_eq!(rec.fields["amount"], Value::Decimal("99.90".into()));
        assert!(lookup.lookup_by_key("people", "ext_id", "nope").await.unwrap().is_none());
        let by_id = lookup.lookup_by_key("people", "id", &dest).await.unwrap().expect("by id");
        assert_eq!(by_id.fields["ext_id"], Value::String("u1".into()));

        let err =
            sink.update_fields("people", "999999", BTreeMap::from([("name".to_string(), Value::String("x".into()))])).await.unwrap_err();
        assert_eq!(err.class, ErrorClass::NotFound);
    })
    .await;
}

/// Regression: keyset pagination over integer keys must use numeric order. The select list
/// emits `"id"::text`, whose output name shadows the column in ORDER BY; reading 25 rows in
/// pages of 10 used to skip/repeat rows at the 9→10 boundary.
#[tokio::test(flavor = "multi_thread")]
async fn keyset_pagination_uses_numeric_order() {
    with_db(|h, url| async move {
        exec(&url, "CREATE TABLE seq (id serial PRIMARY KEY, v text); INSERT INTO seq (v) SELECT 'r' || g FROM generate_series(1, 25) g;")
            .await;
        let src = h.source.clone().unwrap();
        let mut ids = vec![];
        let mut cursor = None;
        loop {
            let b = src.read_batch("seq", cursor.as_ref(), 10).await.unwrap();
            ids.extend(b.records.iter().map(|r| r.id.parse::<i64>().unwrap()));
            match b.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(ids, (1..=25).collect::<Vec<_>>());
    })
    .await;
}

/// Regression: binding to `char(n)` must not truncate (a `::character` cast means char(1)).
#[tokio::test(flavor = "multi_thread")]
async fn char_columns_are_not_truncated() {
    with_db(|h, url| async move {
        exec(&url, "CREATE TABLE money (k text PRIMARY KEY, ccy char(3) NOT NULL);").await;
        let sink = h.sink.clone().unwrap();
        let spec = SinkObjectSpec {
            object: "money".into(),
            fields: vec![FieldDef::new("k", FieldType::String), FieldDef::new("ccy", FieldType::String)],
            key_field: Some("k".into()),
            mode: WriteMode::Upsert,
        };
        // reload the catalog so the new table is visible
        let conn = PostgresConnector::connect(&serde_json::json!({ "url": url }), None).await.unwrap();
        let h2 = handle(conn);
        let sink2 = h2.sink.clone().unwrap();
        let _ = sink;
        sink2.prepare(&spec, None).await.unwrap();
        let out = sink2
            .write_batch("money", vec![req("1", &[("k", Value::String("a".into())), ("ccy", Value::String("SEK".into()))])])
            .await
            .unwrap();
        assert!(out[0].is_ok());
        let b = h2.source.clone().unwrap().read_batch("money", None, 10).await.unwrap();
        assert_eq!(b.records[0].fields["ccy"], Value::String("SEK".into()));
    })
    .await;
}
