//! PostgreSQL connector: schema discovery, keyset reads, transactional upserts and key lookups.
//!
//! All identifiers interpolated into SQL come from discovered catalog metadata and are quoted
//! with [`quote_ident`]; values are always bound as parameters.

mod catalog;
mod sql;
mod write;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use ma_connector_sdk::{Connector, ConnectorHandle, RecordLookup, RecordSink, RecordSource, SinkObjectSpec, WriteRequest, WriteResult};
use ma_types::{Batch, CapabilityManifest, Cursor, Evidence, MaError, MaResult, Record, SystemIdentity, SystemSchema, Value, WriteMode};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgRow};
use sqlx::{AssertSqlSafe, PgPool, Row};
use tokio::sync::Mutex;

use crate::catalog::{Catalog, TableMeta};
pub use crate::sql::quote_ident;
use crate::sql::{param_expr, pg_err, select_list, table_ref, to_param};

/// Per-object state of a write session.
#[derive(Clone)]
struct SinkState {
    spec: SinkObjectSpec,
    table: Arc<TableMeta>,
    allowed: HashSet<String>,
}

pub struct PostgresConnector {
    pool: PgPool,
    schema: String,
    identity: SystemIdentity,
    catalog: Mutex<Option<Arc<Catalog>>>,
    sinks: Mutex<HashMap<String, SinkState>>,
}

impl std::fmt::Debug for PostgresConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresConnector").field("identity", &self.identity).field("schema", &self.schema).finish()
    }
}

impl PostgresConnector {
    /// settings JSON: `{"url": "postgres://user@host:port/db", "schema": "public"}` (schema
    /// defaults to `public`). `password` is injected into the connect options; the URL itself
    /// must not carry a password.
    pub async fn connect(settings: &serde_json::Value, password: Option<String>) -> MaResult<Self> {
        let raw_url =
            settings.get("url").and_then(|v| v.as_str()).ok_or_else(|| MaError::config("postgres settings require a string `url`"))?;
        let schema = match settings.get("schema") {
            None | Some(serde_json::Value::Null) => "public".to_string(),
            Some(serde_json::Value::String(s)) if !s.is_empty() => s.clone(),
            Some(_) => return Err(MaError::config("postgres setting `schema` must be a non-empty string")),
        };
        let parsed = url::Url::parse(raw_url).map_err(|e| MaError::config(format!("invalid postgres url: {e}")))?;
        if !matches!(parsed.scheme(), "postgres" | "postgresql") {
            return Err(MaError::config("postgres url must use the postgres:// or postgresql:// scheme"));
        }
        if parsed.password().is_some() || parsed.query_pairs().any(|(k, _)| k.eq_ignore_ascii_case("password")) {
            return Err(MaError::config("postgres url must not contain a password; store it as the connection credential"));
        }
        let mut opts = PgConnectOptions::from_str(raw_url)
            .map_err(|e| MaError::config(format!("invalid postgres url: {e}")))?
            .application_name("migrate-anything");
        if let Some(p) = password.as_deref() {
            opts = opts.password(p);
        }
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(30))
            .connect_with(opts)
            .await
            .map_err(|e| pg_err("connect", e))?;

        let db_name: String =
            sqlx::query_scalar("SELECT current_database()::text").fetch_one(&pool).await.map_err(|e| pg_err("reading database name", e))?;
        let version: Option<String> = sqlx::query_scalar("SHOW server_version").fetch_one(&pool).await.ok();
        let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)")
            .bind(&schema)
            .fetch_one(&pool)
            .await
            .map_err(|e| pg_err("checking schema", e))?;
        if !exists {
            return Err(MaError::config(format!("postgres schema `{schema}` does not exist in database `{db_name}`")));
        }

        let mut locator = parsed.clone();
        locator.set_fragment(None);
        let identity = SystemIdentity { kind: "postgres".into(), name: db_name, version, locator: format!("{locator}#{schema}") };
        Ok(PostgresConnector { pool, schema, identity, catalog: Mutex::new(None), sinks: Mutex::new(HashMap::new()) })
    }

    async fn refresh_catalog(&self) -> MaResult<Arc<Catalog>> {
        let cat = Arc::new(catalog::load_catalog(&self.pool, &self.schema).await?);
        *self.catalog.lock().await = Some(cat.clone());
        Ok(cat)
    }

    /// Table metadata from the cache, refreshing once if the table is not known yet.
    async fn table(&self, object: &str) -> MaResult<Arc<TableMeta>> {
        let cached = self.catalog.lock().await.clone();
        if let Some(t) = cached.as_ref().and_then(|c| c.tables.get(object)) {
            return Ok(t.clone());
        }
        let cat = self.refresh_catalog().await?;
        cat.tables.get(object).cloned().ok_or_else(|| MaError::config(format!("table `{object}` not found in schema `{}`", self.schema)))
    }

    fn tref(&self, t: &TableMeta) -> String {
        table_ref(&self.schema, &t.name)
    }

    /// Expression yielding the record identifier for rows of `t`.
    fn id_expr(t: &TableMeta) -> String {
        match t.primary_key.as_slice() {
            [one] => format!("{}::text", quote_ident(one)),
            [] => "ctid::text".to_string(),
            many => format!("json_build_array({})::text", many.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", ")),
        }
    }

    /// Convert a row produced by `SELECT <select_list>, <id>` into a record.
    fn row_to_record(&self, t: &TableMeta, row: &PgRow) -> MaResult<Record> {
        let n = t.columns.len();
        let id: Option<String> = row.try_get(n).map_err(|e| pg_err("decoding row id", e))?;
        let mut rec = Record::new(t.name.clone(), id.unwrap_or_default());
        for (i, c) in t.columns.iter().enumerate() {
            let text: Option<String> = row.try_get(i).map_err(|e| pg_err("decoding row", e))?;
            let v = match &text {
                None => Value::Null,
                Some(s) => Value::parse_as(s, c.field_type),
            };
            match c.name.as_str() {
                "created_at" => rec.provenance.source_created_at = text.clone(),
                "updated_at" => rec.provenance.source_updated_at = text.clone(),
                _ => {}
            }
            rec.fields.insert(c.name.clone(), v);
        }
        rec.provenance.system = self.identity.name.clone();
        rec.provenance.extracted_at = Some(Utc::now());
        Ok(rec)
    }
}

/// Wrap one connector instance into a handle exposing every capability.
pub fn handle(c: PostgresConnector) -> ConnectorHandle {
    let c = Arc::new(c);
    ConnectorHandle { connector: c.clone(), source: Some(c.clone()), sink: Some(c.clone()), lookup: Some(c) }
}

#[async_trait]
impl Connector for PostgresConnector {
    fn identify(&self) -> SystemIdentity {
        self.identity.clone()
    }

    fn capabilities(&self) -> CapabilityManifest {
        let mut m = CapabilityManifest::new(self.identity.clone());
        let native = || Some("native SQL implementation".to_string());
        for key in ["read", "write", "update", "transactions", "lookup_by_key", "schema_discovery"] {
            m.set(key, Evidence::Verified, native());
        }
        m.set("pagination", Evidence::Verified, Some("keyset on single-column primary keys, OFFSET otherwise".into()));
        m.set("upsert", Evidence::Verified, Some("INSERT ... ON CONFLICT (key) DO UPDATE".into()));
        m.set("attachments", Evidence::Unsupported, None);
        m.limitations
            .push("tables without a single-column primary key are read with OFFSET pagination (unstable under concurrent writes)".into());
        m.limitations
            .push("upserts require the key field to be covered by a single-column primary key, unique constraint or unique index".into());
        m.limitations.push("only tables of the configured schema are discovered; views are not".into());
        m
    }

    async fn check_connection(&self) -> MaResult<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await.map_err(|e| pg_err("connection check", e))?;
        Ok(())
    }

    async fn discover_schema(&self) -> MaResult<SystemSchema> {
        let cat = self.refresh_catalog().await?;
        catalog::system_schema(&self.pool, &self.schema, &cat).await
    }

    fn connector_version(&self) -> String {
        concat!("builtin-", env!("CARGO_PKG_VERSION")).to_string()
    }
}

#[async_trait]
impl RecordSource for PostgresConnector {
    async fn read_batch(&self, object: &str, cursor: Option<&Cursor>, limit: usize) -> MaResult<Batch> {
        let t = self.table(object).await?;
        let limit = limit.max(1);
        let list = select_list(&t);
        let tref = self.tref(&t);
        let rows = if let Some(pk) = t.single_pk() {
            let after = match cursor {
                None | Some(serde_json::Value::Null) => None,
                Some(c) => Some(
                    c.get("after")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| MaError::config("postgres cursor must be {\"after\": \"<key>\"}"))?
                        .to_string(),
                ),
            };
            let q = quote_ident(&pk.name);
            match after {
                None => {
                    let sql = format!("SELECT {list}, {q}::text FROM {tref} AS ma_t ORDER BY ma_t.{q} LIMIT $1");
                    sqlx::query(AssertSqlSafe(sql)).bind(limit as i64).fetch_all(&self.pool).await
                }
                Some(a) => {
                    let sql = format!(
                        "SELECT {list}, {q}::text FROM {tref} AS ma_t WHERE ma_t.{q} > $1::{} ORDER BY ma_t.{q} LIMIT $2",
                        pk.cast_type
                    );
                    sqlx::query(AssertSqlSafe(sql)).bind(a).bind(limit as i64).fetch_all(&self.pool).await
                }
            }
        } else {
            let offset = match cursor {
                None | Some(serde_json::Value::Null) => 0,
                Some(c) => {
                    c.get("offset").and_then(|v| v.as_u64()).ok_or_else(|| MaError::config("postgres cursor must be {\"offset\": n}"))?
                }
            };
            let order = if t.primary_key.is_empty() {
                "ma_t.ctid".to_string()
            } else {
                t.primary_key.iter().map(|c| format!("ma_t.{}", quote_ident(c))).collect::<Vec<_>>().join(", ")
            };
            let sql = format!("SELECT {list}, {} FROM {tref} AS ma_t ORDER BY {order} LIMIT $1 OFFSET $2", Self::id_expr(&t));
            let rows = sqlx::query(AssertSqlSafe(sql)).bind(limit as i64).bind(offset as i64).fetch_all(&self.pool).await;
            let rows = rows.map_err(|e| pg_err("reading rows", e))?;
            let records = rows.iter().map(|r| self.row_to_record(&t, r)).collect::<MaResult<Vec<_>>>()?;
            let next = (records.len() >= limit).then(|| serde_json::json!({ "offset": offset + records.len() as u64 }));
            return Ok(Batch { records, next_cursor: next });
        };
        let rows = rows.map_err(|e| pg_err("reading rows", e))?;
        let records = rows.iter().map(|r| self.row_to_record(&t, r)).collect::<MaResult<Vec<_>>>()?;
        let next = if records.len() >= limit { records.last().map(|r| serde_json::json!({ "after": r.id })) } else { None };
        Ok(Batch { records, next_cursor: next })
    }

    async fn count(&self, object: &str) -> MaResult<Option<u64>> {
        let t = self.table(object).await?;
        Ok(Some(catalog::exact_count(&self.pool, &self.schema, &t.name).await?))
    }
}

#[async_trait]
impl RecordSink for PostgresConnector {
    async fn prepare(&self, spec: &SinkObjectSpec, _resume_token: Option<&Cursor>) -> MaResult<()> {
        let cat = self.refresh_catalog().await?;
        let table = cat
            .tables
            .get(&spec.object)
            .cloned()
            .ok_or_else(|| MaError::config(format!("destination table `{}` not found in schema `{}`", spec.object, self.schema)))?;
        let mut allowed = HashSet::new();
        for f in &spec.fields {
            let c = table
                .col(&f.name)
                .ok_or_else(|| MaError::config(format!("destination table `{}` has no column `{}`", table.name, f.name)))?;
            if c.generated {
                return Err(MaError::config(format!("column `{}`.`{}` is generated and cannot be written", table.name, c.name)));
            }
            allowed.insert(f.name.clone());
        }
        if let Some(k) = &spec.key_field {
            if table.col(k).is_none() {
                return Err(MaError::config(format!("destination table `{}` has no key column `{k}`", table.name)));
            }
            allowed.insert(k.clone());
        }
        if spec.mode == WriteMode::Upsert {
            let Some(k) = &spec.key_field else {
                return Err(MaError::config(format!("upsert into `{}` requires a key field", table.name)));
            };
            if !table.is_unique_column(k) {
                return Err(MaError::config(format!(
                    "upsert key `{k}` of `{}` is not covered by a single-column primary key, unique constraint or unique index",
                    table.name
                )));
            }
        }
        self.sinks.lock().await.insert(spec.object.clone(), SinkState { spec: spec.clone(), table, allowed });
        Ok(())
    }

    async fn write_batch(&self, object: &str, records: Vec<WriteRequest>) -> MaResult<Vec<WriteResult>> {
        let st =
            self.sinks.lock().await.get(object).cloned().ok_or_else(|| MaError::config(format!("write to `{object}` before prepare")))?;
        write::write_batch(&self.pool, &self.schema, &st.spec, &st.table, &st.allowed, records).await
    }

    async fn durable_token(&self, _object: &str) -> MaResult<Option<Cursor>> {
        Ok(None)
    }

    async fn update_fields(&self, object: &str, dest_id: &str, fields: BTreeMap<String, Value>) -> MaResult<()> {
        let t = self.table(object).await?;
        let pk =
            t.single_pk().ok_or_else(|| MaError::unsupported(format!("updates require a single-column primary key on `{}`", t.name)))?;
        if fields.is_empty() {
            return Ok(());
        }
        let mut sets = Vec::with_capacity(fields.len());
        let mut params = Vec::with_capacity(fields.len() + 1);
        for (name, v) in &fields {
            let c = t.col(name).ok_or_else(|| MaError::validation(format!("table `{}` has no column `{name}`", t.name)))?;
            if c.read_only() {
                return Err(MaError::validation(format!("column `{name}` of `{}` is read-only", t.name)));
            }
            params.push(to_param(v, c)?);
            sets.push(format!("{} = {}", quote_ident(&c.name), param_expr(c, params.len())));
        }
        params.push(Some(dest_id.to_string()));
        let sql = format!(
            "UPDATE {} SET {} WHERE {} = ${}::{}",
            self.tref(&t),
            sets.join(", "),
            quote_ident(&pk.name),
            params.len(),
            pk.cast_type
        );
        let mut q = sqlx::query(AssertSqlSafe(sql));
        for p in params {
            q = q.bind(p);
        }
        let res = q.execute(&self.pool).await.map_err(|e| pg_err("updating row", e))?;
        if res.rows_affected() == 0 {
            return Err(MaError::not_found(format!("no row in `{}` with {} = {dest_id}", t.name, pk.name)));
        }
        Ok(())
    }

    async fn finish(&self, _object: &str) -> MaResult<()> {
        Ok(())
    }

    fn idempotent_rewrites(&self, mode: WriteMode, has_key: bool) -> bool {
        mode == WriteMode::Upsert && has_key
    }

    fn supports_updates(&self) -> bool {
        true
    }
}

#[async_trait]
impl RecordLookup for PostgresConnector {
    async fn lookup_by_key(&self, object: &str, key_field: &str, key: &str) -> MaResult<Option<Record>> {
        let t = self.table(object).await?;
        let c = t.col(key_field).ok_or_else(|| MaError::config(format!("table `{}` has no column `{key_field}`", t.name)))?;
        let sql = format!(
            "SELECT {}, {} FROM {} WHERE {}::text = $1 LIMIT 2",
            select_list(&t),
            Self::id_expr(&t),
            self.tref(&t),
            quote_ident(&c.name)
        );
        let rows = sqlx::query(AssertSqlSafe(sql)).bind(key).fetch_all(&self.pool).await.map_err(|e| pg_err("looking up row", e))?;
        rows.first().map(|r| self.row_to_record(&t, r)).transpose()
    }
}
