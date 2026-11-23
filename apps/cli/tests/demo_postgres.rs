//! Demo 3 — Database migration between two isolated PostgreSQL databases with schema
//! differences (renamed tables/columns, type changes, surrogate keys) and foreign-key
//! dependencies. Re-running an approved plan must not create duplicates (idempotent upserts).

mod common;

use common::*;
use ma_core::{Actor, SpawnMode};
use ma_types::*;

const SOURCE_SQL: &str = r#"
CREATE TABLE customers (cust_no int PRIMARY KEY, cust_name varchar(80) NOT NULL, country char(2) NOT NULL,
  credit_limit numeric(12,2), active boolean NOT NULL DEFAULT true, meta jsonb);
CREATE TABLE orders (order_no bigint PRIMARY KEY, cust_no int NOT NULL REFERENCES customers(cust_no),
  ordered_on date NOT NULL, status varchar(12) NOT NULL, total numeric(14,2) NOT NULL, created_at timestamptz NOT NULL);
CREATE TABLE order_items (item_id serial PRIMARY KEY, order_no bigint NOT NULL REFERENCES orders(order_no),
  sku text NOT NULL, qty int NOT NULL, unit_price numeric(12,2) NOT NULL);
INSERT INTO customers SELECT g, 'Customer ' || g, (ARRAY['SE','DE','US','JP'])[1 + g % 4], (g * 137 % 100000) / 4.0,
  g % 9 <> 0, jsonb_build_object('segment', (ARRAY['smb','ent'])[1 + g % 2], 'score', g % 7 + 0.5) FROM generate_series(1, 300) g;
INSERT INTO orders SELECT 100000 + g, 1 + g % 300, date '2024-01-01' + (g % 365), (ARRAY['NEW','SHIPPED','CANCELLED'])[1 + g % 3],
  (g * 7919 % 1000000) / 100.0, timestamptz '2024-01-01 08:00:00+01' + g * interval '37 minutes' FROM generate_series(1, 1500) g;
INSERT INTO order_items (order_no, sku, qty, unit_price) SELECT 100000 + 1 + g % 1500, 'SKU-' || (g % 97), 1 + g % 5, (g * 31 % 10000) / 100.0
  FROM generate_series(1, 4000) g;
"#;

const DEST_SQL: &str = r#"
CREATE TABLE clients (id bigserial PRIMARY KEY, ext_ref text UNIQUE NOT NULL, display_name text NOT NULL, country_code char(2) NOT NULL,
  credit numeric(14,2), is_active boolean NOT NULL, attributes jsonb);
CREATE TABLE purchase_orders (id bigserial PRIMARY KEY, ext_ref text UNIQUE NOT NULL, client_id bigint NOT NULL REFERENCES clients(id),
  order_date date NOT NULL, state text NOT NULL, amount numeric(14,2) NOT NULL, source_created_at timestamptz);
CREATE TABLE lines (id bigserial PRIMARY KEY, ext_ref text UNIQUE NOT NULL, order_id bigint NOT NULL REFERENCES purchase_orders(id),
  product_code text NOT NULL, quantity int NOT NULL, price numeric(12,2) NOT NULL);
"#;

fn mapping() -> MappingSpec {
    MappingSpec {
        mapping_version: 1,
        objects: vec![
            object(
                "customers",
                "clients",
                Some("ext_ref"),
                vec![
                    fx("ext_ref", cast("cust_no", FieldType::String)),
                    f("display_name", "cust_name"),
                    f("country_code", "country"),
                    f("credit", "credit_limit"),
                    f("is_active", "active"),
                    f("attributes", "meta"),
                ],
            ),
            object(
                "orders",
                "purchase_orders",
                Some("ext_ref"),
                vec![
                    fx("ext_ref", cast("order_no", FieldType::String)),
                    reff("client_id", "cust_no", "customers"),
                    f("order_date", "ordered_on"),
                    fx("state", Expr::Lower { input: Box::new(Expr::field("status")) }),
                    f("amount", "total"),
                    f("source_created_at", "created_at"),
                ],
            ),
            object(
                "order_items",
                "lines",
                Some("ext_ref"),
                vec![
                    fx("ext_ref", cast("item_id", FieldType::String)),
                    reff("order_id", "order_no", "orders"),
                    f("product_code", "sku"),
                    f("quantity", "qty"),
                    f("price", "unit_price"),
                ],
            ),
        ],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn demo3_postgres_to_postgres() {
    let Some(admin) = pg_admin_url() else { return };
    let env = TestEnv::new().await;
    let src = TempDb::create(&admin, "ma_src").await;
    let dst = TempDb::create(&admin, "ma_dst").await;
    src.exec(SOURCE_SQL).await;
    dst.exec(DEST_SQL).await;
    env.pg_conn("legacy-db", &src.url, Environment::Production).await;
    env.pg_conn("new-db", &dst.url, Environment::Production).await;

    let d = env.app.discover("legacy-db", Actor::Cli).await.unwrap();
    assert_eq!(d.schema.object("orders").unwrap().estimated_count, Some(1500));
    assert!(
        d.schema
            .relationships
            .iter()
            .any(|r| r.from_object == "order_items" && r.to_object == "orders" && r.evidence == Evidence::Documented)
    );

    // the heuristic suggester pairs differently named tables via FK structure and synonyms
    let s = env.app.suggest_mapping("legacy-db", "new-db", &[]).await.unwrap();
    let pair = |x: &str| s.pairings.iter().find(|p| p.source_object == x).map(|p| p.target_object.clone());
    assert_eq!(pair("customers").as_deref(), Some("clients"));

    let plan = env.plan("legacy-db", "new-db", mapping(), 400).await;
    assert!(plan.approvable, "{:?}", plan.blocking_risks);
    assert_eq!(plan.execution_order, vec!["customers", "orders", "order_items"]);

    let (_, report, v) = env.execute(&plan.plan_id).await;
    assert_eq!(report.records_written, 300 + 1500 + 4000);
    assert!(v.discrepancies.is_empty(), "{:?}", v.discrepancies);
    assert_eq!(report.final_status, FinalStatus::CompletedAndVerified);
    assert_eq!(report.relationships_preserved, 1500 + 4000);

    // json and booleans survive
    assert_eq!(dst.scalar_i64("SELECT count(*) FROM clients WHERE attributes ? 'segment'").await, 300);
    assert_eq!(dst.scalar_i64("SELECT count(*) FROM clients WHERE NOT is_active").await, 33);
    // same sums on both sides (numeric equality, not float)
    let sum_src = src.scalar_i64("SELECT (sum(total) * 100)::bigint FROM orders").await;
    let sum_dst = dst.scalar_i64("SELECT (sum(amount) * 100)::bigint FROM purchase_orders").await;
    assert_eq!(sum_src, sum_dst);
    assert_eq!(dst.scalar_i64("SELECT count(*) FROM lines l JOIN purchase_orders p ON p.id = l.order_id").await, 4000);

    // re-run the same approved plan: idempotent upserts, no duplicates
    let job2 = env.app.start_job(&plan.plan_id, SpawnMode::Inline, Actor::Cli).await.unwrap();
    env.app.run_worker(&job2.job_id).await.unwrap();
    assert_eq!(dst.scalar_i64("SELECT count(*) FROM purchase_orders").await, 1500);
    assert_eq!(dst.scalar_i64("SELECT count(*) FROM lines").await, 4000);
    let v2 = env.app.verify_job(&job2.job_id, Actor::Cli).await.unwrap();
    assert!(v2.discrepancies.is_empty(), "{:?}", v2.discrepancies);

    src.drop_db().await;
    dst.drop_db().await;
}
