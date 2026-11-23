//! Atomic batch commit: outcomes, id mappings, exceptions, counters and the checkpoint are
//! stored in one SQLite transaction, so job state never claims more than was written.

use ma_types::{Cursor, MaError, OutcomeKind, StageCounters};

use crate::{DbExt, DbResult, ExceptionRow, Store, now, to_json};

pub struct IdMapEntry {
    pub source_id: String,
    pub dest_id: String,
    pub dest_key: Option<String>,
}

pub struct BatchCommit<'a> {
    pub job_id: &'a str,
    pub stage: &'a str,
    pub source_object: &'a str,
    /// Cursor to resume from (`None` = keep the current one; used by second passes).
    pub next_cursor: Option<Cursor>,
    pub sink_token: Option<Cursor>,
    pub id_maps: Vec<IdMapEntry>,
    pub outcomes: Vec<(String, OutcomeKind, Option<String>)>,
    pub exceptions: Vec<ExceptionRow>,
    pub counters: &'a StageCounters,
    pub stage_state: &'a str,
}

impl Store {
    pub async fn commit_batch(&self, c: BatchCommit<'_>) -> DbResult<()> {
        let ts = now();
        let mut tx = self.pool.begin().await.db()?;
        // multi-row inserts: 150 rows x 6 params stays below SQLite's parameter limit
        for chunk in c.id_maps.chunks(150) {
            let values = vec!["(?,?,?,?,?,?)"; chunk.len()].join(",");
            let sql = format!(
                "INSERT OR REPLACE INTO id_mappings (workspace_id, job_id, source_object, source_id, dest_id, dest_key) VALUES {values}"
            );
            let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
            for m in chunk {
                q = q
                    .bind(&self.ws)
                    .bind(c.job_id)
                    .bind(c.source_object)
                    .bind(&m.source_id)
                    .bind(&m.dest_id)
                    .bind(m.dest_key.as_ref().unwrap_or(&m.dest_id));
            }
            q.execute(&mut *tx).await.db()?;
        }
        for (id, kind, msg) in &c.outcomes {
            sqlx::query(
                "INSERT INTO record_outcomes (workspace_id, job_id, source_object, source_id, outcome, message, created_at) VALUES (?,?,?,?,?,?,?)",
            )
            .bind(&self.ws)
            .bind(c.job_id)
            .bind(c.source_object)
            .bind(id)
            .bind(kind.as_str())
            .bind(msg)
            .bind(&ts)
            .execute(&mut *tx)
            .await
            .db()?;
        }
        for e in &c.exceptions {
            sqlx::query(
                "INSERT INTO exceptions (workspace_id, job_id, source_object, source_id, kind, field, value_json, reason, created_at) VALUES (?,?,?,?,?,?,?,?,?)",
            )
            .bind(&self.ws)
            .bind(c.job_id)
            .bind(&e.source_object)
            .bind(&e.source_id)
            .bind(&e.kind)
            .bind(&e.field)
            .bind(e.value.as_ref().map(|v| v.to_string()))
            .bind(&e.reason)
            .bind(&ts)
            .execute(&mut *tx)
            .await
            .db()?;
        }
        let cursor_json = c.next_cursor.as_ref().map(to_json);
        let token_json = c.sink_token.as_ref().map(to_json);
        sqlx::query(
            "INSERT INTO checkpoints (workspace_id, job_id, stage, cursor_json, sink_token_json, inflight_json, batches, updated_at)
             VALUES (?,?,?,?,?,NULL,1,?)
             ON CONFLICT (workspace_id, job_id, stage) DO UPDATE SET
               cursor_json=COALESCE(excluded.cursor_json, checkpoints.cursor_json),
               sink_token_json=COALESCE(excluded.sink_token_json, checkpoints.sink_token_json),
               inflight_json=NULL, batches=checkpoints.batches+1, updated_at=excluded.updated_at",
        )
        .bind(&self.ws)
        .bind(c.job_id)
        .bind(c.stage)
        .bind(cursor_json)
        .bind(token_json)
        .bind(&ts)
        .execute(&mut *tx)
        .await
        .db()?;
        let r = sqlx::query("UPDATE job_stages SET counters_json=?, state=?, updated_at=? WHERE workspace_id=? AND job_id=? AND stage=?")
            .bind(to_json(c.counters))
            .bind(c.stage_state)
            .bind(&ts)
            .bind(&self.ws)
            .bind(c.job_id)
            .bind(c.stage)
            .execute(&mut *tx)
            .await
            .db()?;
        if r.rows_affected() != 1 {
            return Err(MaError::internal(format!("unknown stage {} for job {}", c.stage, c.job_id)));
        }
        tx.commit().await.db()?;
        Ok(())
    }
}
