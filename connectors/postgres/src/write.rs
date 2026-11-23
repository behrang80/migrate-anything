//! Transactional batch writes.

use std::collections::{BTreeMap, HashMap, HashSet};

use ma_connector_sdk::{SinkObjectSpec, WriteRequest, WriteResult};
use ma_types::{ErrorClass, MaError, MaResult, WriteMode};
use sqlx::{AssertSqlSafe, PgConnection, PgPool, Row};

use crate::catalog::TableMeta;
use crate::sql::{param_expr, pg_err, quote_ident, table_ref, to_param};

/// Stay well below PostgreSQL's 65535 bind-parameter limit.
const MAX_PARAMS: usize = 30_000;

struct PreparedRow {
    idx: usize,
    /// Column names in table order.
    cols: Vec<String>,
    params: Vec<Option<String>>,
    key_text: Option<String>,
}

struct Ctx<'a> {
    schema: &'a str,
    table: &'a TableMeta,
    key: Option<&'a str>,
}

impl Ctx<'_> {
    fn id_expr(&self) -> String {
        if let Some(pk) = self.table.single_pk() {
            format!("{}::text", quote_ident(&pk.name))
        } else if let Some(k) = self.key {
            format!("{}::text", quote_ident(k))
        } else {
            "ctid::text".into()
        }
    }

    /// `INSERT ... VALUES (..),(..) [ON CONFLICT] RETURNING id, key`.
    fn insert_sql(&self, cols: &[String], nrows: usize, upsert: bool) -> String {
        let tref = table_ref(self.schema, &self.table.name);
        let key_ret = match self.key {
            Some(k) => format!("{}::text", quote_ident(k)),
            None => "NULL::text".into(),
        };
        let returning = format!("RETURNING {}, {key_ret}", self.id_expr());
        if cols.is_empty() {
            return format!("INSERT INTO {tref} DEFAULT VALUES {returning}");
        }
        let metas: Vec<_> = cols.iter().filter_map(|c| self.table.col(c)).collect();
        let overriding = if metas.iter().any(|c| c.identity_always()) { " OVERRIDING SYSTEM VALUE" } else { "" };
        let col_list = cols.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", ");
        let mut n = 0;
        let values = (0..nrows)
            .map(|_| {
                let exprs = metas
                    .iter()
                    .map(|c| {
                        n += 1;
                        param_expr(c, n)
                    })
                    .collect::<Vec<_>>();
                format!("({})", exprs.join(", "))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut sql = format!("INSERT INTO {tref} ({col_list}){overriding} VALUES {values}");
        if upsert && let Some(k) = self.key {
            let mut sets: Vec<String> = metas
                .iter()
                .filter(|c| c.name != k && !c.identity_always())
                .map(|c| format!("{0} = EXCLUDED.{0}", quote_ident(&c.name)))
                .collect();
            if sets.is_empty() {
                // A no-op update so RETURNING still yields the existing row.
                sets.push(format!("{0} = EXCLUDED.{0}", quote_ident(k)));
            }
            sql.push_str(&format!(" ON CONFLICT ({}) DO UPDATE SET {}", quote_ident(k), sets.join(", ")));
        }
        sql.push(' ');
        sql.push_str(&returning);
        sql
    }
}

fn prepare_row(
    idx: usize,
    req: &WriteRequest,
    table: &TableMeta,
    allowed: &HashSet<String>,
    spec: &SinkObjectSpec,
) -> MaResult<PreparedRow> {
    if let Some(unknown) = req.fields.keys().find(|k| !allowed.contains(*k)) {
        return Err(MaError::validation(format!("field `{unknown}` is not part of the prepared destination fields of `{}`", table.name)));
    }
    let mut cols = Vec::new();
    let mut params = Vec::new();
    let mut key_text = None;
    for c in &table.columns {
        let Some(v) = req.fields.get(&c.name) else { continue };
        if c.generated {
            return Err(MaError::validation(format!("column `{}` is generated and cannot be written", c.name)));
        }
        let p = to_param(v, c)?;
        if spec.key_field.as_deref() == Some(c.name.as_str()) {
            key_text = p.clone();
        }
        cols.push(c.name.clone());
        params.push(p);
    }
    if spec.mode == WriteMode::Upsert && key_text.is_none() {
        return Err(MaError::validation(format!("record has no value for upsert key `{}`", spec.key_field.as_deref().unwrap_or_default())));
    }
    Ok(PreparedRow { idx, cols, params, key_text })
}

pub(crate) async fn write_batch(
    pool: &PgPool,
    schema: &str,
    spec: &SinkObjectSpec,
    table: &TableMeta,
    allowed: &HashSet<String>,
    records: Vec<WriteRequest>,
) -> MaResult<Vec<WriteResult>> {
    let mut results: Vec<Option<WriteResult>> = vec![None; records.len()];
    let mut rows = Vec::with_capacity(records.len());
    for (i, r) in records.iter().enumerate() {
        match prepare_row(i, r, table, allowed, spec) {
            Ok(p) => rows.push(p),
            Err(e) => results[i] = Some(Err(e)),
        }
    }
    let ctx = Ctx { schema, table, key: spec.key_field.as_deref() };
    let upsert = spec.mode == WriteMode::Upsert;

    let mut tx = pool.begin().await.map_err(|e| pg_err("beginning transaction", e))?;
    if upsert {
        // One statement per set of present columns, chunked by parameter count, never
        // repeating a key inside one statement (ON CONFLICT cannot touch a row twice).
        let mut groups: BTreeMap<Vec<String>, Vec<PreparedRow>> = BTreeMap::new();
        for r in rows {
            groups.entry(r.cols.clone()).or_default().push(r);
        }
        for (cols, group) in groups {
            let max_rows = (MAX_PARAMS / cols.len().max(1)).max(1);
            let mut chunk: Vec<PreparedRow> = Vec::new();
            let mut keys: HashSet<String> = HashSet::new();
            for r in group {
                let k = r.key_text.clone().unwrap_or_default();
                if chunk.len() >= max_rows || keys.contains(&k) {
                    exec_chunk(&mut tx, &ctx, &cols, std::mem::take(&mut chunk), &mut results).await?;
                    keys.clear();
                }
                keys.insert(k);
                chunk.push(r);
            }
            if !chunk.is_empty() {
                exec_chunk(&mut tx, &ctx, &cols, chunk, &mut results).await?;
            }
        }
    } else {
        for r in &rows {
            exec_single(&mut tx, &ctx, r, false, &mut results).await?;
        }
    }
    tx.commit().await.map_err(|e| pg_err("committing batch", e))?;

    Ok(results.into_iter().map(|r| r.unwrap_or_else(|| Err(MaError::internal("record produced no write result")))).collect())
}

async fn exec(conn: &mut PgConnection, sql: &'static str) -> MaResult<()> {
    sqlx::query(sql).execute(conn).await.map_err(|e| pg_err("savepoint", e))?;
    Ok(())
}

async fn exec_chunk(
    conn: &mut PgConnection,
    ctx: &Ctx<'_>,
    cols: &[String],
    chunk: Vec<PreparedRow>,
    results: &mut [Option<WriteResult>],
) -> MaResult<()> {
    if chunk.len() == 1 {
        return exec_single(conn, ctx, &chunk[0], true, results).await;
    }
    exec(conn, "SAVEPOINT ma_chunk").await?;
    let sql = ctx.insert_sql(cols, chunk.len(), true);
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for r in &chunk {
        for p in &r.params {
            q = q.bind(p.clone());
        }
    }
    match q.fetch_all(&mut *conn).await {
        Ok(returned) => {
            exec(conn, "RELEASE SAVEPOINT ma_chunk").await?;
            let mut by_key: HashMap<String, String> = HashMap::new();
            let mut ordered = Vec::with_capacity(returned.len());
            for row in &returned {
                let id: Option<String> = row.try_get(0).map_err(|e| pg_err("decoding returned id", e))?;
                let key: Option<String> = row.try_get(1).map_err(|e| pg_err("decoding returned key", e))?;
                let id = id.unwrap_or_default();
                if let Some(k) = key {
                    by_key.insert(k, id.clone());
                }
                ordered.push(id);
            }
            for (j, r) in chunk.iter().enumerate() {
                let id = r
                    .key_text
                    .as_ref()
                    .and_then(|k| by_key.get(k).cloned())
                    // Keys canonicalised by the server (e.g. `1.0` -> `1.00`): INSERT ... VALUES
                    // returns rows in input order.
                    .or_else(|| (ordered.len() == chunk.len()).then(|| ordered[j].clone()));
                results[r.idx] = Some(id.ok_or_else(|| MaError::internal("could not match upserted row to its request")));
            }
            Ok(())
        }
        Err(e) => {
            let err = pg_err("writing batch", e);
            if err.class != ErrorClass::Validation {
                return Err(err);
            }
            // Isolate the failing rows: retry each one in its own savepoint.
            exec(conn, "ROLLBACK TO SAVEPOINT ma_chunk").await?;
            for r in &chunk {
                exec_single(conn, ctx, r, true, results).await?;
            }
            exec(conn, "RELEASE SAVEPOINT ma_chunk").await?;
            Ok(())
        }
    }
}

async fn exec_single(
    conn: &mut PgConnection,
    ctx: &Ctx<'_>,
    r: &PreparedRow,
    upsert: bool,
    results: &mut [Option<WriteResult>],
) -> MaResult<()> {
    exec(conn, "SAVEPOINT ma_row").await?;
    let sql = ctx.insert_sql(&r.cols, 1, upsert);
    let mut q = sqlx::query(AssertSqlSafe(sql));
    for p in &r.params {
        q = q.bind(p.clone());
    }
    match q.fetch_optional(&mut *conn).await {
        Ok(row) => {
            exec(conn, "RELEASE SAVEPOINT ma_row").await?;
            let id = match row {
                Some(row) => row.try_get::<Option<String>, _>(0).map_err(|e| pg_err("decoding returned id", e))?.unwrap_or_default(),
                None => return Err(MaError::internal("insert returned no row")),
            };
            results[r.idx] = Some(Ok(id));
            Ok(())
        }
        Err(e) => {
            let err = pg_err("writing record", e);
            if err.class != ErrorClass::Validation {
                return Err(err);
            }
            exec(conn, "ROLLBACK TO SAVEPOINT ma_row").await?;
            exec(conn, "RELEASE SAVEPOINT ma_row").await?;
            results[r.idx] = Some(Err(err));
            Ok(())
        }
    }
}
