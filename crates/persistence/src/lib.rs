//! Durable workspace state (SQLite, WAL) with embedded, versioned migrations.
//!
//! All methods take and return `ma-types` structures, so a PostgreSQL implementation can be
//! added behind the same API without touching domain logic.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ma_types::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};

pub mod batch;
pub use batch::*;

pub type DbResult<T> = Result<T, MaError>;

fn db_err(e: sqlx::Error) -> MaError {
    match &e {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) => MaError::transient(format!("state database: {e}")),
        _ => MaError::internal(format!("state database: {e}")),
    }
}

trait DbExt<T> {
    fn db(self) -> DbResult<T>;
}
impl<T> DbExt<T> for Result<T, sqlx::Error> {
    fn db(self) -> DbResult<T> {
        self.map_err(db_err)
    }
}

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339()
}

pub(crate) fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("serializable")
}

pub(crate) fn from_json<T: DeserializeOwned>(s: &str) -> DbResult<T> {
    serde_json::from_str(s).map_err(|e| MaError::internal(format!("corrupt stored JSON: {e}")))
}

fn parse_ts(s: Option<String>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()).map(|d| d.with_timezone(&Utc))
}

#[derive(Clone)]
pub struct Store {
    pub(crate) pool: SqlitePool,
    pub(crate) ws: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectorVersionRow {
    pub connector_id: String,
    pub version: String,
    pub status: ConnectorStatus,
    pub spec: DeclarativeConnectorSpec,
    pub spec_sha256: String,
    pub manifest: CapabilityManifest,
    pub created_at: String,
    pub activated_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredPlan {
    pub plan: MigrationPlan,
    pub plan_hash: String,
    pub summary_md: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct Checkpoint {
    pub cursor: Option<Cursor>,
    pub sink_token: Option<Cursor>,
    pub inflight: Vec<String>,
    pub batches: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    pub at: String,
    pub actor: String,
    pub action: String,
    pub subject: Option<String>,
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExceptionRow {
    pub source_object: String,
    pub source_id: String,
    pub kind: String,
    pub field: Option<String>,
    pub value: Option<serde_json::Value>,
    pub reason: String,
}

impl Store {
    /// Open (creating if needed) the state database and apply pending migrations.
    pub async fn open(path: &Path, workspace: &str) -> DbResult<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(30))
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await.db()?;
        sqlx::migrate!("./migrations").run(&pool).await.map_err(|e| MaError::internal(format!("state migration failed: {e}")))?;
        sqlx::query("INSERT OR IGNORE INTO workspaces (id, created_at) VALUES (?, ?)")
            .bind(workspace)
            .bind(now())
            .execute(&pool)
            .await
            .db()?;
        Ok(Store { pool, ws: workspace.to_string() })
    }

    pub fn workspace(&self) -> &str {
        &self.ws
    }

    pub async fn schema_version(&self) -> DbResult<i64> {
        let v: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations").fetch_one(&self.pool).await.db()?;
        Ok(v.unwrap_or(0))
    }

    // ---------------------------------------------------------------- audit
    pub async fn audit(&self, actor: &str, action: &str, subject: Option<&str>, detail: serde_json::Value) -> DbResult<()> {
        sqlx::query("INSERT INTO audit_events (workspace_id, at, actor, action, subject, detail_json) VALUES (?,?,?,?,?,?)")
            .bind(&self.ws)
            .bind(now())
            .bind(actor)
            .bind(action)
            .bind(subject)
            .bind(detail.to_string())
            .execute(&self.pool)
            .await
            .db()?;
        Ok(())
    }

    pub async fn audit_events(&self, limit: i64) -> DbResult<Vec<AuditEvent>> {
        let rows =
            sqlx::query("SELECT at, actor, action, subject, detail_json FROM audit_events WHERE workspace_id=? ORDER BY id DESC LIMIT ?")
                .bind(&self.ws)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
                .db()?;
        rows.into_iter()
            .map(|r| {
                Ok(AuditEvent {
                    at: r.get(0),
                    actor: r.get(1),
                    action: r.get(2),
                    subject: r.get(3),
                    detail: from_json(&r.get::<String, _>(4))?,
                })
            })
            .collect()
    }

    // ---------------------------------------------------------------- connections
    pub async fn put_connection(&self, c: &ConnectionConfig) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO connections (workspace_id, name, kind, environment, config_json, created_at, updated_at)
             VALUES (?,?,?,?,?,?,?)
             ON CONFLICT (workspace_id, name) DO UPDATE SET kind=excluded.kind, environment=excluded.environment,
             config_json=excluded.config_json, updated_at=excluded.updated_at",
        )
        .bind(&self.ws)
        .bind(&c.name)
        .bind(&c.kind)
        .bind(c.environment.to_string())
        .bind(to_json(c))
        .bind(now())
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    pub async fn get_connection(&self, name: &str) -> DbResult<ConnectionConfig> {
        let row = sqlx::query("SELECT config_json FROM connections WHERE workspace_id=? AND name=?")
            .bind(&self.ws)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .db()?
            .ok_or_else(|| MaError::not_found(format!("connection `{name}` not found")))?;
        from_json(&row.get::<String, _>(0))
    }

    pub async fn list_connections(&self) -> DbResult<Vec<ConnectionConfig>> {
        let rows = sqlx::query("SELECT config_json FROM connections WHERE workspace_id=? ORDER BY name")
            .bind(&self.ws)
            .fetch_all(&self.pool)
            .await
            .db()?;
        rows.iter().map(|r| from_json(&r.get::<String, _>(0))).collect()
    }

    // ---------------------------------------------------------------- systems & schemas
    pub async fn put_system(&self, connection: &str, identity: &SystemIdentity, manifest: &CapabilityManifest) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO systems (workspace_id, connection, identity_json, manifest_json, discovered_at) VALUES (?,?,?,?,?)
             ON CONFLICT (workspace_id, connection) DO UPDATE SET identity_json=excluded.identity_json,
             manifest_json=excluded.manifest_json, discovered_at=excluded.discovered_at",
        )
        .bind(&self.ws)
        .bind(connection)
        .bind(to_json(identity))
        .bind(to_json(manifest))
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    pub async fn put_schema(&self, connection: &str, schema: &SystemSchema) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO schemas (workspace_id, connection, schema_json, discovered_at) VALUES (?,?,?,?)
             ON CONFLICT (workspace_id, connection) DO UPDATE SET schema_json=excluded.schema_json, discovered_at=excluded.discovered_at",
        )
        .bind(&self.ws)
        .bind(connection)
        .bind(to_json(schema))
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    // ---------------------------------------------------------------- credentials
    pub async fn put_credential(&self, name: &str, ciphertext: &str) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO credentials (workspace_id, name, ciphertext, created_at) VALUES (?,?,?,?)
             ON CONFLICT (workspace_id, name) DO UPDATE SET ciphertext=excluded.ciphertext",
        )
        .bind(&self.ws)
        .bind(name)
        .bind(ciphertext)
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    pub async fn get_credential(&self, name: &str) -> DbResult<String> {
        sqlx::query_scalar("SELECT ciphertext FROM credentials WHERE workspace_id=? AND name=?")
            .bind(&self.ws)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .db()?
            .ok_or_else(|| MaError::not_found(format!("credential `{name}` not found")))
    }

    pub async fn list_credentials(&self) -> DbResult<Vec<String>> {
        sqlx::query_scalar("SELECT name FROM credentials WHERE workspace_id=? ORDER BY name")
            .bind(&self.ws)
            .fetch_all(&self.pool)
            .await
            .db()
    }

    // ---------------------------------------------------------------- connectors
    pub async fn put_connector_version(
        &self,
        spec: &DeclarativeConnectorSpec,
        manifest: &CapabilityManifest,
        status: ConnectorStatus,
    ) -> DbResult<String> {
        let sha = hash::canonical_sha256(spec);
        let mut tx = self.pool.begin().await.db()?;
        sqlx::query("INSERT OR IGNORE INTO connectors (workspace_id, connector_id, created_at) VALUES (?,?,?)")
            .bind(&self.ws)
            .bind(&spec.connector_id)
            .bind(now())
            .execute(&mut *tx)
            .await
            .db()?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT spec_sha256 FROM connector_versions WHERE workspace_id=? AND connector_id=? AND version=?")
                .bind(&self.ws)
                .bind(&spec.connector_id)
                .bind(&spec.version)
                .fetch_optional(&mut *tx)
                .await
                .db()?;
        match existing {
            Some(s) if s == sha => {
                // identical spec already registered: keep its status
            }
            Some(_) => {
                return Err(MaError::config(format!(
                    "connector {}@{} already exists with different content; bump the version",
                    spec.connector_id, spec.version
                )));
            }
            None => {
                sqlx::query(
                    "INSERT INTO connector_versions (workspace_id, connector_id, version, status, spec_json, spec_sha256, manifest_json, created_at)
                     VALUES (?,?,?,?,?,?,?,?)",
                )
                .bind(&self.ws)
                .bind(&spec.connector_id)
                .bind(&spec.version)
                .bind(status.to_string())
                .bind(to_json(spec))
                .bind(&sha)
                .bind(to_json(manifest))
                .bind(now())
                .execute(&mut *tx)
                .await
                .db()?;
            }
        }
        tx.commit().await.db()?;
        Ok(sha)
    }

    pub async fn update_connector_manifest(&self, id: &str, version: &str, manifest: &CapabilityManifest) -> DbResult<()> {
        sqlx::query("UPDATE connector_versions SET manifest_json=? WHERE workspace_id=? AND connector_id=? AND version=?")
            .bind(to_json(manifest))
            .bind(&self.ws)
            .bind(id)
            .bind(version)
            .execute(&self.pool)
            .await
            .db()?;
        Ok(())
    }

    pub async fn set_connector_status(&self, id: &str, version: &str, status: ConnectorStatus, by: Option<&str>) -> DbResult<()> {
        let r = sqlx::query(
            "UPDATE connector_versions SET status=?, activated_by=COALESCE(?, activated_by),
             activated_at=CASE WHEN ? IS NULL THEN activated_at ELSE ? END
             WHERE workspace_id=? AND connector_id=? AND version=?",
        )
        .bind(status.to_string())
        .bind(by)
        .bind(by)
        .bind(now())
        .bind(&self.ws)
        .bind(id)
        .bind(version)
        .execute(&self.pool)
        .await
        .db()?;
        if r.rows_affected() == 0 {
            return Err(MaError::not_found(format!("connector {id}@{version} not found")));
        }
        Ok(())
    }

    fn row_to_connector(r: &sqlx::sqlite::SqliteRow) -> DbResult<ConnectorVersionRow> {
        Ok(ConnectorVersionRow {
            connector_id: r.get(0),
            version: r.get(1),
            status: ConnectorStatus::from_str(&r.get::<String, _>(2)).map_err(MaError::internal)?,
            spec: from_json(&r.get::<String, _>(3))?,
            spec_sha256: r.get(4),
            manifest: from_json(&r.get::<String, _>(5))?,
            created_at: r.get(6),
            activated_by: r.get(7),
        })
    }

    pub async fn get_connector_version(&self, id: &str, version: Option<&str>) -> DbResult<ConnectorVersionRow> {
        let row = match version {
            Some(v) => sqlx::query(
                "SELECT connector_id, version, status, spec_json, spec_sha256, manifest_json, created_at, activated_by
                 FROM connector_versions WHERE workspace_id=? AND connector_id=? AND version=?",
            )
            .bind(&self.ws)
            .bind(id)
            .bind(v)
            .fetch_optional(&self.pool)
            .await
            .db()?,
            None => sqlx::query(
                "SELECT connector_id, version, status, spec_json, spec_sha256, manifest_json, created_at, activated_by
                 FROM connector_versions WHERE workspace_id=? AND connector_id=? ORDER BY created_at DESC LIMIT 1",
            )
            .bind(&self.ws)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .db()?,
        };
        let row = row.ok_or_else(|| MaError::not_found(format!("connector {id}@{} not found", version.unwrap_or("latest"))))?;
        Self::row_to_connector(&row)
    }

    pub async fn list_connector_versions(&self) -> DbResult<Vec<ConnectorVersionRow>> {
        let rows = sqlx::query(
            "SELECT connector_id, version, status, spec_json, spec_sha256, manifest_json, created_at, activated_by
             FROM connector_versions WHERE workspace_id=? ORDER BY connector_id, created_at",
        )
        .bind(&self.ws)
        .fetch_all(&self.pool)
        .await
        .db()?;
        rows.iter().map(Self::row_to_connector).collect()
    }

    pub async fn add_connector_test(&self, id: &str, version: &str, connection: &str, results: &[ContractTestResult]) -> DbResult<()> {
        let passed = results.iter().all(|r| r.passed);
        sqlx::query(
            "INSERT INTO connector_tests (workspace_id, connector_id, version, connection, results_json, passed, tested_at) VALUES (?,?,?,?,?,?,?)",
        )
        .bind(&self.ws)
        .bind(id)
        .bind(version)
        .bind(connection)
        .bind(to_json(&results))
        .bind(passed as i64)
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    pub async fn latest_connector_test(&self, id: &str, version: &str) -> DbResult<Option<(bool, Vec<ContractTestResult>)>> {
        let row = sqlx::query(
            "SELECT passed, results_json FROM connector_tests WHERE workspace_id=? AND connector_id=? AND version=? ORDER BY id DESC LIMIT 1",
        )
        .bind(&self.ws)
        .bind(id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await
        .db()?;
        match row {
            None => Ok(None),
            Some(r) => Ok(Some((r.get::<i64, _>(0) != 0, from_json(&r.get::<String, _>(1))?))),
        }
    }

    // ---------------------------------------------------------------- extensions
    pub async fn put_extension(&self, name: &str, sha256: &str, module: &[u8], status: &str) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO extensions (workspace_id, name, sha256, module, status, created_at) VALUES (?,?,?,?,?,?)
             ON CONFLICT (workspace_id, name) DO UPDATE SET sha256=excluded.sha256, module=excluded.module, status=excluded.status",
        )
        .bind(&self.ws)
        .bind(name)
        .bind(sha256)
        .bind(module)
        .bind(status)
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    pub async fn get_extension(&self, name: &str) -> DbResult<(String, Vec<u8>, String)> {
        let r = sqlx::query("SELECT sha256, module, status FROM extensions WHERE workspace_id=? AND name=?")
            .bind(&self.ws)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .db()?
            .ok_or_else(|| MaError::not_found(format!("extension `{name}` not found")))?;
        Ok((r.get(0), r.get(1), r.get(2)))
    }

    // ---------------------------------------------------------------- plans
    pub async fn put_plan(&self, plan: &MigrationPlan, plan_hash: &str, summary_md: &str) -> DbResult<()> {
        sqlx::query("INSERT INTO plans (workspace_id, plan_id, plan_hash, plan_json, summary_md, created_at) VALUES (?,?,?,?,?,?)")
            .bind(&self.ws)
            .bind(&plan.plan_id)
            .bind(plan_hash)
            .bind(to_json(plan))
            .bind(summary_md)
            .bind(now())
            .execute(&self.pool)
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() => {
                    MaError::config("plans are immutable: a plan with this id already exists")
                }
                _ => db_err(e),
            })?;
        Ok(())
    }

    pub async fn get_plan(&self, plan_id: &str) -> DbResult<StoredPlan> {
        let r = sqlx::query("SELECT plan_json, plan_hash, summary_md, created_at FROM plans WHERE workspace_id=? AND plan_id=?")
            .bind(&self.ws)
            .bind(plan_id)
            .fetch_optional(&self.pool)
            .await
            .db()?
            .ok_or_else(|| MaError::not_found(format!("plan `{plan_id}` not found")))?;
        Ok(StoredPlan { plan: from_json(&r.get::<String, _>(0))?, plan_hash: r.get(1), summary_md: r.get(2), created_at: r.get(3) })
    }

    pub async fn list_plans(&self) -> DbResult<Vec<(String, String, String)>> {
        let rows = sqlx::query("SELECT plan_id, plan_hash, created_at FROM plans WHERE workspace_id=? ORDER BY created_at DESC")
            .bind(&self.ws)
            .fetch_all(&self.pool)
            .await
            .db()?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1), r.get(2))).collect())
    }

    pub async fn put_dry_run(&self, report: &DryRunReport, plan_id: &str) -> DbResult<()> {
        sqlx::query("INSERT INTO dry_runs (workspace_id, plan_id, plan_hash, report_json, created_at) VALUES (?,?,?,?,?)")
            .bind(&self.ws)
            .bind(plan_id)
            .bind(&report.plan_hash)
            .bind(to_json(report))
            .bind(now())
            .execute(&self.pool)
            .await
            .db()?;
        Ok(())
    }

    pub async fn latest_dry_run(&self, plan_id: &str) -> DbResult<Option<DryRunReport>> {
        let r: Option<String> =
            sqlx::query_scalar("SELECT report_json FROM dry_runs WHERE workspace_id=? AND plan_id=? ORDER BY id DESC LIMIT 1")
                .bind(&self.ws)
                .bind(plan_id)
                .fetch_optional(&self.pool)
                .await
                .db()?;
        r.map(|s| from_json(&s)).transpose()
    }

    // ---------------------------------------------------------------- approvals
    pub async fn request_approval(&self, plan_id: &str, plan_hash: &str, requested_by: &str, note: Option<&str>) -> DbResult<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO approval_requests (workspace_id, plan_id, plan_hash, requested_by, note, requested_at) VALUES (?,?,?,?,?,?)",
        )
        .bind(&self.ws)
        .bind(plan_id)
        .bind(plan_hash)
        .bind(requested_by)
        .bind(note)
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    pub async fn has_approval_request(&self, plan_id: &str, plan_hash: &str) -> DbResult<bool> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM approval_requests WHERE workspace_id=? AND plan_id=? AND plan_hash=?")
            .bind(&self.ws)
            .bind(plan_id)
            .bind(plan_hash)
            .fetch_one(&self.pool)
            .await
            .db()?;
        Ok(n > 0)
    }

    pub async fn put_approval(&self, rec: &ApprovalRecord) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO approvals (workspace_id, approval_id, plan_id, plan_hash, record_json, created_at, revoked) VALUES (?,?,?,?,?,?,0)",
        )
        .bind(&self.ws)
        .bind(&rec.claims.approval_id)
        .bind(&rec.claims.plan_id)
        .bind(&rec.claims.plan_hash)
        .bind(to_json(rec))
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    pub async fn approvals_for_plan(&self, plan_id: &str) -> DbResult<Vec<ApprovalRecord>> {
        let rows = sqlx::query("SELECT record_json, revoked FROM approvals WHERE workspace_id=? AND plan_id=? ORDER BY created_at DESC")
            .bind(&self.ws)
            .bind(plan_id)
            .fetch_all(&self.pool)
            .await
            .db()?;
        rows.iter()
            .map(|r| {
                let mut rec: ApprovalRecord = from_json(&r.get::<String, _>(0))?;
                rec.revoked = r.get::<i64, _>(1) != 0;
                Ok(rec)
            })
            .collect()
    }

    pub async fn get_approval(&self, approval_id: &str) -> DbResult<ApprovalRecord> {
        let r = sqlx::query("SELECT record_json, revoked FROM approvals WHERE workspace_id=? AND approval_id=?")
            .bind(&self.ws)
            .bind(approval_id)
            .fetch_optional(&self.pool)
            .await
            .db()?
            .ok_or_else(|| MaError::not_found(format!("approval `{approval_id}` not found")))?;
        let mut rec: ApprovalRecord = from_json(&r.get::<String, _>(0))?;
        rec.revoked = r.get::<i64, _>(1) != 0;
        Ok(rec)
    }

    pub async fn revoke_approval(&self, approval_id: &str) -> DbResult<()> {
        sqlx::query("UPDATE approvals SET revoked=1 WHERE workspace_id=? AND approval_id=?")
            .bind(&self.ws)
            .bind(approval_id)
            .execute(&self.pool)
            .await
            .db()?;
        Ok(())
    }

    // ---------------------------------------------------------------- jobs
    pub async fn create_job(&self, job_id: &str, plan: &MigrationPlan, plan_hash: &str, approval_id: &str) -> DbResult<()> {
        let mut tx = self.pool.begin().await.db()?;
        sqlx::query(
            "INSERT INTO jobs (workspace_id, job_id, plan_id, plan_hash, approval_id, status, control, created_at) VALUES (?,?,?,?,?,'pending','none',?)",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(&plan.plan_id)
        .bind(plan_hash)
        .bind(approval_id)
        .bind(now())
        .execute(&mut *tx)
        .await
        .db()?;
        let mut stages: Vec<String> = plan.execution_order.clone();
        if !plan.deferred_references.is_empty() {
            for o in &plan.execution_order {
                if plan.deferred_references.iter().any(|d| &d.source_object == o) {
                    stages.push(format!("{o}#pass2"));
                }
            }
        }
        for (seq, stage) in stages.iter().enumerate() {
            sqlx::query(
                "INSERT INTO job_stages (workspace_id, job_id, stage, seq, state, counters_json, updated_at) VALUES (?,?,?,?,'pending',?,?)",
            )
            .bind(&self.ws)
            .bind(job_id)
            .bind(stage)
            .bind(seq as i64)
            .bind(to_json(&StageCounters::default()))
            .bind(now())
            .execute(&mut *tx)
            .await
            .db()?;
        }
        tx.commit().await.db()?;
        Ok(())
    }

    pub async fn get_job(&self, job_id: &str) -> DbResult<JobInfo> {
        let r = sqlx::query(
            "SELECT job_id, plan_id, plan_hash, status, control, created_at, started_at, finished_at, heartbeat_at, worker_pid, error, attempts
             FROM jobs WHERE workspace_id=? AND job_id=?",
        )
        .bind(&self.ws)
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
        .db()?
        .ok_or_else(|| MaError::not_found(format!("job `{job_id}` not found")))?;
        let stages = self.job_stages(job_id).await?;
        Ok(JobInfo {
            job_id: r.get(0),
            plan_id: r.get(1),
            plan_hash: r.get(2),
            status: JobStatus::from_str(&r.get::<String, _>(3)).map_err(MaError::internal)?,
            control: match r.get::<String, _>(4).as_str() {
                "pause" => JobControl::Pause,
                "cancel" => JobControl::Cancel,
                _ => JobControl::None,
            },
            created_at: parse_ts(Some(r.get(5))).unwrap_or_else(Utc::now),
            started_at: parse_ts(r.get(6)),
            finished_at: parse_ts(r.get(7)),
            heartbeat_at: parse_ts(r.get(8)),
            worker_pid: r.get::<Option<i64>, _>(9).map(|p| p as u32),
            error: r.get(10),
            stages,
            attempts: r.get::<i64, _>(11) as u32,
        })
    }

    pub async fn job_approval_id(&self, job_id: &str) -> DbResult<String> {
        sqlx::query_scalar("SELECT approval_id FROM jobs WHERE workspace_id=? AND job_id=?")
            .bind(&self.ws)
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await
            .db()?
            .ok_or_else(|| MaError::not_found(format!("job `{job_id}` not found")))
    }

    /// Bind a job to another approval of the same plan. Used when the approval a job started
    /// under expired or was revoked while the job was not running and the operator has
    /// approved the plan again.
    pub async fn set_job_approval(&self, job_id: &str, approval_id: &str) -> DbResult<()> {
        sqlx::query("UPDATE jobs SET approval_id=? WHERE workspace_id=? AND job_id=?")
            .bind(approval_id)
            .bind(&self.ws)
            .bind(job_id)
            .execute(&self.pool)
            .await
            .db()?;
        Ok(())
    }

    pub async fn list_jobs(&self) -> DbResult<Vec<(String, String, String, String)>> {
        let rows = sqlx::query("SELECT job_id, plan_id, status, created_at FROM jobs WHERE workspace_id=? ORDER BY created_at DESC")
            .bind(&self.ws)
            .fetch_all(&self.pool)
            .await
            .db()?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1), r.get(2), r.get(3))).collect())
    }

    pub async fn job_stages(&self, job_id: &str) -> DbResult<Vec<StageProgress>> {
        let rows = sqlx::query("SELECT stage, state, counters_json FROM job_stages WHERE workspace_id=? AND job_id=? ORDER BY seq")
            .bind(&self.ws)
            .bind(job_id)
            .fetch_all(&self.pool)
            .await
            .db()?;
        rows.iter()
            .map(|r| Ok(StageProgress { source_object: r.get(0), state: r.get(1), counters: from_json(&r.get::<String, _>(2))? }))
            .collect()
    }

    pub async fn set_stage_state(&self, job_id: &str, stage: &str, state: &str) -> DbResult<()> {
        sqlx::query("UPDATE job_stages SET state=?, updated_at=? WHERE workspace_id=? AND job_id=? AND stage=?")
            .bind(state)
            .bind(now())
            .bind(&self.ws)
            .bind(job_id)
            .bind(stage)
            .execute(&self.pool)
            .await
            .db()?;
        Ok(())
    }

    pub async fn set_job_status(&self, job_id: &str, status: JobStatus, error: Option<&str>) -> DbResult<()> {
        let finished = if status.is_terminal() { Some(now()) } else { None };
        sqlx::query(
            "UPDATE jobs SET status=?1,
             error=CASE WHEN ?1 IN ('pending','running','completed') THEN ?2 ELSE COALESCE(?2, error) END,
             finished_at=COALESCE(?3, finished_at),
             started_at=CASE WHEN ?1='running' AND started_at IS NULL THEN ?4 ELSE started_at END
             WHERE workspace_id=?5 AND job_id=?6",
        )
        .bind(status.as_str())
        .bind(error)
        .bind(finished)
        .bind(now())
        .bind(&self.ws)
        .bind(job_id)
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    pub async fn set_job_control(&self, job_id: &str, control: JobControl) -> DbResult<()> {
        let c = match control {
            JobControl::None => "none",
            JobControl::Pause => "pause",
            JobControl::Cancel => "cancel",
        };
        sqlx::query("UPDATE jobs SET control=? WHERE workspace_id=? AND job_id=?")
            .bind(c)
            .bind(&self.ws)
            .bind(job_id)
            .execute(&self.pool)
            .await
            .db()?;
        Ok(())
    }

    /// Acquire or renew the job lease. Succeeds if the lease is free, expired, or already ours.
    pub async fn acquire_lease(&self, job_id: &str, owner: &str, lease: Duration, pid: u32) -> DbResult<bool> {
        let now_ms = Utc::now().timestamp_millis();
        let r = sqlx::query(
            "UPDATE jobs SET lease_owner=?, lease_expires_at=?, heartbeat_at=?, worker_pid=?,
             attempts = attempts + CASE WHEN lease_owner IS ? THEN 0 ELSE 1 END
             WHERE workspace_id=? AND job_id=? AND (lease_owner IS NULL OR lease_owner=? OR lease_expires_at < ?)",
        )
        .bind(owner)
        .bind(now_ms + lease.as_millis() as i64)
        .bind(now())
        .bind(pid as i64)
        .bind(owner)
        .bind(&self.ws)
        .bind(job_id)
        .bind(owner)
        .bind(now_ms)
        .execute(&self.pool)
        .await
        .db()?;
        Ok(r.rows_affected() == 1)
    }

    pub async fn release_lease(&self, job_id: &str, owner: &str) -> DbResult<()> {
        sqlx::query("UPDATE jobs SET lease_owner=NULL, lease_expires_at=NULL WHERE workspace_id=? AND job_id=? AND lease_owner=?")
            .bind(&self.ws)
            .bind(job_id)
            .bind(owner)
            .execute(&self.pool)
            .await
            .db()?;
        Ok(())
    }

    pub async fn lease_state(&self, job_id: &str) -> DbResult<(Option<String>, Option<i64>)> {
        let r = sqlx::query("SELECT lease_owner, lease_expires_at FROM jobs WHERE workspace_id=? AND job_id=?")
            .bind(&self.ws)
            .bind(job_id)
            .fetch_one(&self.pool)
            .await
            .db()?;
        Ok((r.get(0), r.get(1)))
    }

    // ---------------------------------------------------------------- checkpoints
    pub async fn get_checkpoint(&self, job_id: &str, stage: &str) -> DbResult<Checkpoint> {
        let r = sqlx::query(
            "SELECT cursor_json, sink_token_json, inflight_json, batches FROM checkpoints WHERE workspace_id=? AND job_id=? AND stage=?",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(stage)
        .fetch_optional(&self.pool)
        .await
        .db()?;
        let Some(r) = r else { return Ok(Checkpoint::default()) };
        Ok(Checkpoint {
            cursor: r.get::<Option<String>, _>(0).map(|s| from_json(&s)).transpose()?,
            sink_token: r.get::<Option<String>, _>(1).map(|s| from_json(&s)).transpose()?,
            inflight: r.get::<Option<String>, _>(2).map(|s| from_json(&s)).transpose()?.unwrap_or_default(),
            batches: r.get::<i64, _>(3) as u64,
        })
    }

    /// Record the ids of a batch about to be written (before any destination I/O).
    /// Store the sink position reached right after `prepare` (e.g. a file's length after its
    /// header), so a job that stops before its first batch commit can still be resumed.
    pub async fn set_initial_sink_token(&self, job_id: &str, stage: &str, token: &Cursor) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO checkpoints (workspace_id, job_id, stage, sink_token_json, updated_at) VALUES (?,?,?,?,?)
             ON CONFLICT (workspace_id, job_id, stage) DO UPDATE SET
               sink_token_json=COALESCE(checkpoints.sink_token_json, excluded.sink_token_json), updated_at=excluded.updated_at",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(stage)
        .bind(to_json(token))
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    pub async fn mark_inflight(&self, job_id: &str, stage: &str, ids: &[String]) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO checkpoints (workspace_id, job_id, stage, inflight_json, updated_at) VALUES (?,?,?,?,?)
             ON CONFLICT (workspace_id, job_id, stage) DO UPDATE SET inflight_json=excluded.inflight_json, updated_at=excluded.updated_at",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(stage)
        .bind(to_json(&ids))
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    // ---------------------------------------------------------------- id map
    pub async fn lookup_dest_ids(&self, job_id: &str, source_object: &str, source_ids: &[String]) -> DbResult<Vec<(String, String)>> {
        let mut out = vec![];
        for chunk in source_ids.chunks(400) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT source_id, dest_id FROM id_mappings WHERE workspace_id=? AND job_id=? AND source_object=? AND source_id IN ({placeholders})"
            );
            let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(&self.ws).bind(job_id).bind(source_object);
            for id in chunk {
                q = q.bind(id);
            }
            for r in q.fetch_all(&self.pool).await.db()? {
                out.push((r.get(0), r.get(1)));
            }
        }
        Ok(out)
    }

    /// Which of `keys` are destination keys (dest_key, or dest_id when no key) of this job.
    pub async fn keys_in_id_map(&self, job_id: &str, source_object: &str, keys: &[String]) -> DbResult<Vec<(String, String)>> {
        let mut out = vec![];
        for chunk in keys.chunks(400) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT dest_key, source_id FROM id_mappings WHERE workspace_id=? AND job_id=? AND source_object=?
                 AND dest_key IN ({placeholders})"
            );
            let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(&self.ws).bind(job_id).bind(source_object);
            for k in chunk {
                q = q.bind(k);
            }
            for r in q.fetch_all(&self.pool).await.db()? {
                out.push((r.get(0), r.get(1)));
            }
        }
        Ok(out)
    }

    pub async fn id_map_count(&self, job_id: &str, source_object: &str) -> DbResult<u64> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM id_mappings WHERE workspace_id=? AND job_id=? AND source_object=?")
            .bind(&self.ws)
            .bind(job_id)
            .bind(source_object)
            .fetch_one(&self.pool)
            .await
            .db()?;
        Ok(n as u64)
    }

    /// Page through id mappings (source_id, dest_id) ordered by source_id.
    pub async fn id_map_page(&self, job_id: &str, source_object: &str, after: Option<&str>, limit: i64) -> DbResult<Vec<(String, String)>> {
        let rows = sqlx::query(
            "SELECT source_id, dest_id FROM id_mappings WHERE workspace_id=? AND job_id=? AND source_object=? AND source_id > ?
             ORDER BY source_id LIMIT ?",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(source_object)
        .bind(after.unwrap_or(""))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .db()?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    // ---------------------------------------------------------------- outcomes & exceptions
    pub async fn outcome_counts(&self, job_id: &str) -> DbResult<Vec<(String, String, i64)>> {
        let rows = sqlx::query(
            "SELECT source_object, outcome, COUNT(*) FROM record_outcomes WHERE workspace_id=? AND job_id=? GROUP BY source_object, outcome",
        )
        .bind(&self.ws)
        .bind(job_id)
        .fetch_all(&self.pool)
        .await
        .db()?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1), r.get(2))).collect())
    }

    pub async fn record_outcomes(&self, job_id: &str, limit: i64) -> DbResult<Vec<(String, String, String, Option<String>)>> {
        let rows = sqlx::query(
            "SELECT source_object, source_id, outcome, message FROM record_outcomes WHERE workspace_id=? AND job_id=? ORDER BY id LIMIT ?",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .db()?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1), r.get(2), r.get(3))).collect())
    }

    pub async fn exceptions(&self, job_id: &str, limit: i64) -> DbResult<Vec<ExceptionRow>> {
        let rows = sqlx::query(
            "SELECT source_object, source_id, kind, field, value_json, reason FROM exceptions WHERE workspace_id=? AND job_id=? ORDER BY id LIMIT ?",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .db()?;
        rows.iter()
            .map(|r| {
                Ok(ExceptionRow {
                    source_object: r.get(0),
                    source_id: r.get(1),
                    kind: r.get(2),
                    field: r.get(3),
                    value: r.get::<Option<String>, _>(4).map(|s| from_json(&s)).transpose()?,
                    reason: r.get(5),
                })
            })
            .collect()
    }

    pub async fn exception_counts(&self, job_id: &str) -> DbResult<Vec<(String, String, Option<String>, i64)>> {
        let rows = sqlx::query(
            "SELECT source_object, kind, field, COUNT(*) FROM exceptions WHERE workspace_id=? AND job_id=? GROUP BY source_object, kind, field",
        )
        .bind(&self.ws)
        .bind(job_id)
        .fetch_all(&self.pool)
        .await
        .db()?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1), r.get(2), r.get(3))).collect())
    }

    // ---------------------------------------------------------------- validation
    pub async fn put_validation(&self, v: &VerificationResult) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO validation_results (workspace_id, job_id, result_json, created_at) VALUES (?,?,?,?)
             ON CONFLICT (workspace_id, job_id) DO UPDATE SET result_json=excluded.result_json, created_at=excluded.created_at",
        )
        .bind(&self.ws)
        .bind(&v.job_id)
        .bind(to_json(v))
        .bind(now())
        .execute(&self.pool)
        .await
        .db()?;
        Ok(())
    }

    pub async fn get_validation(&self, job_id: &str) -> DbResult<Option<VerificationResult>> {
        let r: Option<String> = sqlx::query_scalar("SELECT result_json FROM validation_results WHERE workspace_id=? AND job_id=?")
            .bind(&self.ws)
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await
            .db()?;
        r.map(|s| from_json(&s)).transpose()
    }

    // verification scratch space
    pub async fn verify_reset(&self, job_id: &str) -> DbResult<()> {
        for sql in
            ["DELETE FROM verify_keys WHERE workspace_id=? AND job_id=?", "DELETE FROM verify_refs WHERE workspace_id=? AND job_id=?"]
        {
            sqlx::query(sql).bind(&self.ws).bind(job_id).execute(&self.pool).await.db()?;
        }
        Ok(())
    }

    pub async fn verify_add_keys(&self, job_id: &str, object: &str, keys: &[String]) -> DbResult<()> {
        let mut tx = self.pool.begin().await.db()?;
        for chunk in keys.chunks(200) {
            let sql =
                format!("INSERT INTO verify_keys (workspace_id, job_id, object, key) VALUES {}", vec!["(?,?,?,?)"; chunk.len()].join(","));
            let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
            for k in chunk {
                q = q.bind(&self.ws).bind(job_id).bind(object).bind(k);
            }
            q.execute(&mut *tx).await.db()?;
        }
        tx.commit().await.db()
    }

    pub async fn verify_add_refs(&self, job_id: &str, object: &str, refs: &[(String, String, String)]) -> DbResult<()> {
        let mut tx = self.pool.begin().await.db()?;
        for (field, ref_object, value) in refs {
            sqlx::query("INSERT INTO verify_refs (workspace_id, job_id, object, field, ref_object, value) VALUES (?,?,?,?,?,?)")
                .bind(&self.ws)
                .bind(job_id)
                .bind(object)
                .bind(field)
                .bind(ref_object)
                .bind(value)
                .execute(&mut *tx)
                .await
                .db()?;
        }
        tx.commit().await.db()
    }

    /// (destination rows seen, distinct keys, id-map keys found in destination, id-map size)
    pub async fn verify_key_stats(&self, job_id: &str, object: &str, source_object: &str) -> DbResult<(u64, u64, u64, u64)> {
        let r = sqlx::query(
            "SELECT
               (SELECT COUNT(*) FROM verify_keys WHERE workspace_id=?1 AND job_id=?2 AND object=?3),
               (SELECT COUNT(DISTINCT key) FROM verify_keys WHERE workspace_id=?1 AND job_id=?2 AND object=?3),
               (SELECT COUNT(*) FROM id_mappings m WHERE m.workspace_id=?1 AND m.job_id=?2 AND m.source_object=?4
                  AND EXISTS (SELECT 1 FROM verify_keys v WHERE v.workspace_id=?1 AND v.job_id=?2 AND v.object=?3 AND v.key = m.dest_key)),
               (SELECT COUNT(*) FROM id_mappings m WHERE m.workspace_id=?1 AND m.job_id=?2 AND m.source_object=?4)",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(object)
        .bind(source_object)
        .fetch_one(&self.pool)
        .await
        .db()?;
        Ok((r.get::<i64, _>(0) as u64, r.get::<i64, _>(1) as u64, r.get::<i64, _>(2) as u64, r.get::<i64, _>(3) as u64))
    }

    /// Keys that appear more than once among destination rows that belong to this job.
    pub async fn verify_duplicate_keys(&self, job_id: &str, object: &str, source_object: &str) -> DbResult<u64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(c - 1), 0) FROM (
               SELECT v.key, COUNT(*) AS c FROM verify_keys v
               WHERE v.workspace_id=?1 AND v.job_id=?2 AND v.object=?3
                 AND EXISTS (SELECT 1 FROM id_mappings m WHERE m.workspace_id=?1 AND m.job_id=?2 AND m.source_object=?4 AND m.dest_key=v.key)
               GROUP BY v.key HAVING COUNT(*) > 1)",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(object)
        .bind(source_object)
        .fetch_one(&self.pool)
        .await
        .db()?;
        Ok(n as u64)
    }

    /// (references checked, references whose value is not a destination id of the referenced object)
    pub async fn verify_ref_stats(&self, job_id: &str, object: &str) -> DbResult<(u64, u64)> {
        let r = sqlx::query(
            "SELECT COUNT(*),
                    SUM(CASE WHEN EXISTS (SELECT 1 FROM id_mappings m WHERE m.workspace_id=r.workspace_id AND m.job_id=r.job_id
                          AND m.source_object=r.ref_object AND m.dest_id=r.value) THEN 0 ELSE 1 END)
             FROM verify_refs r WHERE r.workspace_id=? AND r.job_id=? AND r.object=?",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(object)
        .fetch_one(&self.pool)
        .await
        .db()?;
        Ok((r.get::<i64, _>(0) as u64, r.get::<Option<i64>, _>(1).unwrap_or(0) as u64))
    }

    /// id-map entries whose key was not seen in the destination (up to `limit` examples).
    pub async fn verify_missing_examples(&self, job_id: &str, object: &str, source_object: &str, limit: i64) -> DbResult<Vec<String>> {
        sqlx::query_scalar(
            "SELECT m.source_id FROM id_mappings m WHERE m.workspace_id=?1 AND m.job_id=?2 AND m.source_object=?4
               AND NOT EXISTS (SELECT 1 FROM verify_keys v WHERE v.workspace_id=?1 AND v.job_id=?2 AND v.object=?3 AND v.key = m.dest_key)
             LIMIT ?5",
        )
        .bind(&self.ws)
        .bind(job_id)
        .bind(object)
        .bind(source_object)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .db()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn job_state_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        {
            let s = Store::open(&path, "default").await.unwrap();
            s.audit("tester", "x", None, serde_json::json!({})).await.unwrap();
            sqlx::query("INSERT INTO jobs (workspace_id, job_id, plan_id, plan_hash, approval_id, status, created_at) VALUES ('default','j1','p','h','a','running',?)")
                .bind(now())
                .execute(&s.pool)
                .await
                .unwrap();
            s.pool.close().await;
        }
        let s = Store::open(&path, "default").await.unwrap();
        assert_eq!(s.get_job("j1").await.unwrap().status, JobStatus::Running);
        assert_eq!(s.audit_events(10).await.unwrap().len(), 1);
        assert!(s.schema_version().await.unwrap() >= 1);
    }

    #[tokio::test]
    async fn error_is_cleared_when_a_failed_job_later_completes() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("s.db"), "w").await.unwrap();
        sqlx::query("INSERT INTO jobs (workspace_id, job_id, plan_id, plan_hash, approval_id, status, created_at) VALUES ('w','j','p','h','a','pending',?)")
            .bind(now())
            .execute(&s.pool)
            .await
            .unwrap();
        s.set_job_status("j", JobStatus::Failed, Some("network down")).await.unwrap();
        assert_eq!(s.get_job("j").await.unwrap().error.as_deref(), Some("network down"));
        s.set_job_status("j", JobStatus::Running, None).await.unwrap();
        s.set_job_status("j", JobStatus::Completed, None).await.unwrap();
        let j = s.get_job("j").await.unwrap();
        assert_eq!(j.status, JobStatus::Completed);
        assert_eq!(j.error, None, "stale error must not survive a successful resume");
        assert!(j.started_at.is_some() && j.finished_at.is_some());
    }

    #[tokio::test]
    async fn lease_is_exclusive_until_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("s.db"), "w").await.unwrap();
        sqlx::query("INSERT INTO jobs (workspace_id, job_id, plan_id, plan_hash, approval_id, status, created_at) VALUES ('w','j','p','h','a','pending',?)")
            .bind(now())
            .execute(&s.pool)
            .await
            .unwrap();
        assert!(s.acquire_lease("j", "a", Duration::from_millis(200), 1).await.unwrap());
        assert!(!s.acquire_lease("j", "b", Duration::from_millis(200), 2).await.unwrap());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(s.acquire_lease("j", "b", Duration::from_millis(200), 2).await.unwrap());
    }
}
