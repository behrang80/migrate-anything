//! Migration execution engine.
//!
//! Per object (in dependency order): a reader task prefetches cursor-addressed batches into a
//! bounded channel (backpressure); each batch is transformed, written, and then — in one SQLite
//! transaction — outcomes, id mappings, archived values, counters and the new checkpoint are
//! committed. The job holds a renewable lease; pause/cancel are polled between batches.

pub mod dryrun;
pub mod transform;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ma_connector_sdk::{ConnectorHandle, SinkObjectSpec, WriteRequest};
use ma_mapping::ExtensionRunner;
use ma_persistence::{BatchCommit, ExceptionRow, IdMapEntry, Store};
use ma_types::*;
use serde_json::json;
use tokio::sync::mpsc;

pub use transform::BatchTransformer;

pub struct JobRunner {
    pub store: Store,
    pub plan: MigrationPlan,
    pub plan_hash: String,
    pub source: ConnectorHandle,
    pub destination: ConnectorHandle,
    pub extensions: Option<Arc<dyn ExtensionRunner>>,
    pub job_id: String,
    pub owner: String,
    pub lease: Duration,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    Completed,
    Paused,
    Cancelled,
}

const EXHAUSTED: &str = "__exhausted";

fn is_exhausted(c: &Option<Cursor>) -> bool {
    c.as_ref().and_then(|c| c.get(EXHAUSTED)).is_some()
}

/// Test-only fault injection (`MA_ENABLE_FAULT_INJECTION=1` and `MA_FAULT=<point>:<n>`), used
/// by the interrupted-migration demonstration to kill the worker at precise points.
fn maybe_crash(point: &str, count: u64) {
    if std::env::var("MA_ENABLE_FAULT_INJECTION").as_deref() != Ok("1") {
        return;
    }
    if let Ok(spec) = std::env::var("MA_FAULT")
        && let Some((p, n)) = spec.split_once(':')
        && p == point
        && n.parse::<u64>().ok() == Some(count)
    {
        eprintln!("fault injection: aborting worker at {point}:{count}");
        std::process::exit(86);
    }
}

fn jitter_ms(max: u64) -> u64 {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    if max == 0 { 0 } else { u64::from_le_bytes(b) % max }
}

/// Destination spec for an object mapping.
pub fn sink_spec(plan: &MigrationPlan, om: &ObjectMapping) -> SinkObjectSpec {
    let target = plan.destination_schema.object(&om.target_object);
    let src = plan.source_schema.object(&om.source_object);
    let fields = om
        .fields
        .iter()
        .filter(|f| f.status != ReviewStatus::Rejected)
        .map(|f| {
            if let Some(def) = target.and_then(|t| t.field(&f.target)) {
                return def.clone();
            }
            let ty = match &f.expr {
                Expr::Field { path } => src.and_then(|s| s.field(path)).map(|d| d.field_type).unwrap_or(FieldType::String),
                Expr::Cast { to, .. } => *to,
                Expr::ScaleDecimal { .. } | Expr::UnitConvert { .. } => FieldType::Decimal,
                Expr::ParseDate { datetime, .. } => {
                    if *datetime {
                        FieldType::DateTime
                    } else {
                        FieldType::Date
                    }
                }
                Expr::Object { .. } => FieldType::Object,
                _ => FieldType::String,
            };
            FieldDef::new(f.target.clone(), ty)
        })
        .collect();
    SinkObjectSpec { object: om.target_object.clone(), fields, key_field: om.target_key_field.clone(), mode: om.write_mode }
}

impl JobRunner {
    pub async fn run(&self) -> MaResult<RunEnd> {
        let pid = std::process::id();
        if !self.store.acquire_lease(&self.job_id, &self.owner, self.lease, pid).await? {
            return Err(MaError::policy(format!("job {} is leased by another live worker", self.job_id)));
        }
        let info = self.store.get_job(&self.job_id).await?;
        if info.status.is_terminal() {
            self.store.release_lease(&self.job_id, &self.owner).await?;
            return Err(MaError::config(format!("job {} is already {}", self.job_id, info.status.as_str())));
        }
        self.store.set_job_status(&self.job_id, JobStatus::Running, None).await?;
        self.store.audit(&self.owner, "job.worker_started", Some(&self.job_id), json!({"pid": pid, "attempt": info.attempts})).await?;
        let result = self.run_stages(&info).await;
        let end = match &result {
            Ok(RunEnd::Completed) => {
                self.store.set_job_status(&self.job_id, JobStatus::Completed, None).await?;
                RunEnd::Completed
            }
            Ok(RunEnd::Paused) => {
                self.store.set_job_status(&self.job_id, JobStatus::Paused, None).await?;
                RunEnd::Paused
            }
            Ok(RunEnd::Cancelled) => {
                self.store.set_job_status(&self.job_id, JobStatus::Cancelled, None).await?;
                RunEnd::Cancelled
            }
            Err(e) => {
                self.store.set_job_status(&self.job_id, JobStatus::Failed, Some(&e.to_string())).await?;
                self.store.audit(&self.owner, "job.failed", Some(&self.job_id), json!({"error": e.to_string()})).await.ok();
                self.store.release_lease(&self.job_id, &self.owner).await.ok();
                return Err(e.clone());
            }
        };
        self.store.audit(&self.owner, "job.worker_stopped", Some(&self.job_id), json!({"end": format!("{end:?}")})).await?;
        self.store.release_lease(&self.job_id, &self.owner).await?;
        Ok(end)
    }

    async fn run_stages(&self, info: &JobInfo) -> MaResult<RunEnd> {
        let mut commits = 0u64;
        for stage in &info.stages {
            if stage.state == "done" {
                continue;
            }
            let end = if let Some(obj) = stage.source_object.strip_suffix("#pass2") {
                self.run_second_pass(&stage.source_object, obj, stage.counters.clone(), &mut commits).await?
            } else {
                self.run_object(&stage.source_object, stage.counters.clone(), &mut commits).await?
            };
            if end != RunEnd::Completed {
                return Ok(end);
            }
        }
        Ok(RunEnd::Completed)
    }

    async fn check_control(&self) -> MaResult<Option<RunEnd>> {
        if !self.store.acquire_lease(&self.job_id, &self.owner, self.lease, std::process::id()).await? {
            return Err(MaError::policy("lost the job lease to another worker; stopping"));
        }
        Ok(match self.store.get_job(&self.job_id).await?.control {
            JobControl::Pause => Some(RunEnd::Paused),
            JobControl::Cancel => Some(RunEnd::Cancelled),
            JobControl::None => None,
        })
    }

    async fn write_with_retry(
        &self,
        object: &str,
        reqs: Vec<WriteRequest>,
        counters: &mut StageCounters,
    ) -> MaResult<Vec<MaResult<String>>> {
        let sink = self.destination.sink()?;
        let mut attempt = 0u32;
        loop {
            match sink.write_batch(object, reqs.clone()).await {
                Ok(r) => return Ok(r),
                Err(e) if e.is_retryable() && attempt < self.plan.options.max_retries => {
                    attempt += 1;
                    counters.retries += 1;
                    let base = self.plan.options.retry_base_ms.saturating_mul(1 << attempt.min(10));
                    let wait = e.retry_after.map(|d| d.as_millis() as u64).unwrap_or(base) + jitter_ms(base / 2 + 1);
                    tracing::warn!(object, attempt, wait_ms = wait, error = %e, "retrying batch write");
                    tokio::time::sleep(Duration::from_millis(wait)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn run_object(&self, object: &str, mut counters: StageCounters, commits: &mut u64) -> MaResult<RunEnd> {
        let om = self.plan.mapping.object(object).ok_or_else(|| MaError::internal(format!("no mapping for {object}")))?.clone();
        let target_def = self.plan.destination_schema.object(&om.target_object).cloned();
        let spec = sink_spec(&self.plan, &om);
        let cp = self.store.get_checkpoint(&self.job_id, object).await?;
        let sink = self.destination.sink()?.clone();
        sink.prepare(&spec, cp.sink_token.as_ref()).await?;
        self.store.set_stage_state(&self.job_id, object, "running").await?;
        let has_key = om.target_key_field.is_some();
        let idempotent = sink.idempotent_rewrites(om.write_mode, has_key);
        let mut inflight: Vec<String> = cp.inflight.clone();

        // reader task with bounded prefetch
        let (tx, mut rx) = mpsc::channel::<MaResult<Batch>>(self.plan.options.prefetch_batches.max(1));
        if !is_exhausted(&cp.cursor) {
            let source = self.source.source()?.clone();
            let obj = object.to_string();
            let limit = self.plan.options.batch_size.max(1);
            let start = cp.cursor.clone();
            tokio::spawn(async move {
                let mut cursor = start;
                loop {
                    let mut attempt = 0;
                    let res = loop {
                        match source.read_batch(&obj, cursor.as_ref(), limit).await {
                            Err(e) if e.is_retryable() && attempt < 5 => {
                                attempt += 1;
                                let wait = e.retry_after.map(|d| d.as_millis() as u64).unwrap_or(200 << attempt);
                                tokio::time::sleep(Duration::from_millis(wait)).await;
                            }
                            other => break other,
                        }
                    };
                    let done = match &res {
                        Ok(b) => b.next_cursor.is_none(),
                        Err(_) => true,
                    };
                    if let Ok(b) = &res {
                        cursor = b.next_cursor.clone();
                    }
                    if tx.send(res).await.is_err() || done {
                        return;
                    }
                }
            });
        } else {
            drop(tx);
        }

        let transformer = BatchTransformer::new(&om, target_def.as_ref(), self.extensions.as_deref());
        while let Some(batch) = rx.recv().await {
            if let Some(end) = self.check_control().await? {
                return Ok(end);
            }
            let batch = batch?;
            counters.read += batch.records.len() as u64;
            counters.batches += 1;
            let transformed = transformer.transform(&batch.records, Some((&self.store, &self.job_id)), true).await?;

            let mut outcomes = vec![];
            let mut exceptions = vec![];
            let mut id_maps = vec![];
            let mut to_write: Vec<(usize, WriteRequest)> = vec![];
            for (i, (rec, t)) in batch.records.iter().zip(transformed.iter()).enumerate() {
                if !t.errors.is_empty() || !t.unsupported.is_empty() {
                    let (kind, msg) = if !t.errors.is_empty() {
                        (OutcomeKind::Failed, t.errors.join("; "))
                    } else {
                        (OutcomeKind::Unsupported, t.unsupported.join("; "))
                    };
                    counters.add_outcome(kind, 1);
                    outcomes.push((rec.id.clone(), kind, Some(msg.clone())));
                    exceptions.push(ExceptionRow {
                        source_object: object.into(),
                        source_id: rec.id.clone(),
                        kind: "record_not_written".into(),
                        field: None,
                        value: Some(rec.to_plain_json()),
                        reason: msg,
                    });
                    continue;
                }
                // in-flight records from an interrupted batch on a non-idempotent destination
                if !idempotent && inflight.contains(&rec.id) {
                    let key = om.target_key_field.as_ref().and_then(|k| t.fields.get(k)).and_then(|v| v.to_text());
                    match (&self.destination.lookup, &om.target_key_field, key) {
                        (Some(lookup), Some(kf), Some(key)) => {
                            if let Some(found) = lookup.lookup_by_key(&om.target_object, kf, &key).await? {
                                let dest_id = found.id.clone();
                                id_maps.push(IdMapEntry { source_id: rec.id.clone(), dest_id, dest_key: Some(key) });
                                counters.add_outcome(OutcomeKind::Transformed, 1);
                                continue;
                            }
                        }
                        _ => {
                            counters.add_outcome(OutcomeKind::RequiresReconciliation, 1);
                            outcomes.push((
                                rec.id.clone(),
                                OutcomeKind::RequiresReconciliation,
                                Some("write was in flight when the worker stopped; destination cannot be checked by key".into()),
                            ));
                            continue;
                        }
                    }
                }
                to_write.push((i, WriteRequest { source_id: rec.id.clone(), fields: t.fields.clone() }));
            }
            inflight.clear();

            counters.attempted += to_write.len() as u64;
            if !to_write.is_empty() {
                let ids: Vec<String> = to_write.iter().map(|(_, r)| r.source_id.clone()).collect();
                if !idempotent {
                    self.store.mark_inflight(&self.job_id, object, &ids).await?;
                }
                let reqs: Vec<WriteRequest> = to_write.iter().map(|(_, r)| r.clone()).collect();
                let results = self.write_with_retry(&om.target_object, reqs, &mut counters).await?;
                if results.len() != to_write.len() {
                    return Err(MaError::internal("destination returned a wrong number of results"));
                }
                maybe_crash("after_write", *commits + 1);
                for ((i, req), res) in to_write.into_iter().zip(results) {
                    let rec = &batch.records[i];
                    let t = &transformed[i];
                    match res {
                        Ok(dest_id) => {
                            let kind = if !t.archived.is_empty() {
                                OutcomeKind::WrittenWithArchive
                            } else if t.transformed {
                                OutcomeKind::Transformed
                            } else {
                                OutcomeKind::MigratedNatively
                            };
                            counters.add_outcome(kind, 1);
                            let dest_key = om.target_key_field.as_ref().and_then(|k| req.fields.get(k)).and_then(|v| v.to_text());
                            id_maps.push(IdMapEntry { source_id: rec.id.clone(), dest_id, dest_key });
                            for (field, value, reason) in &t.archived {
                                counters.archived_values += 1;
                                exceptions.push(ExceptionRow {
                                    source_object: object.into(),
                                    source_id: rec.id.clone(),
                                    kind: "archived_field".into(),
                                    field: Some(field.clone()),
                                    value: Some(serde_json::Value::String(ma_types::json_text::to_plain_string(value))),
                                    reason: reason.clone(),
                                });
                            }
                        }
                        Err(e) => {
                            counters.add_outcome(OutcomeKind::Failed, 1);
                            outcomes.push((rec.id.clone(), OutcomeKind::Failed, Some(e.message.clone())));
                            exceptions.push(ExceptionRow {
                                source_object: object.into(),
                                source_id: rec.id.clone(),
                                kind: "write_failed".into(),
                                field: None,
                                value: Some(rec.to_plain_json()),
                                reason: e.message,
                            });
                        }
                    }
                }
            }
            let token = sink.durable_token(&om.target_object).await?;
            let next = Some(batch.next_cursor.clone().unwrap_or_else(|| json!({ EXHAUSTED: true })));
            self.store
                .commit_batch(BatchCommit {
                    job_id: &self.job_id,
                    stage: object,
                    source_object: object,
                    next_cursor: next,
                    sink_token: token,
                    id_maps,
                    outcomes,
                    exceptions,
                    counters: &counters,
                    stage_state: "running",
                })
                .await?;
            *commits += 1;
            maybe_crash("after_commit", *commits);
        }
        sink.finish(&om.target_object).await?;
        self.store.set_stage_state(&self.job_id, object, "done").await?;
        self.store.audit(&self.owner, "job.stage_done", Some(&self.job_id), json!({"object": object, "counters": counters})).await?;
        Ok(RunEnd::Completed)
    }

    /// Second pass: write deferred (cyclic) references now that every object has destination ids.
    async fn run_second_pass(&self, stage: &str, object: &str, mut counters: StageCounters, commits: &mut u64) -> MaResult<RunEnd> {
        let om = self.plan.mapping.object(object).ok_or_else(|| MaError::internal("no mapping"))?;
        let mut deferred_only = om.clone();
        deferred_only.fields.retain(|f| f.deferred);
        deferred_only.unmapped_fields = UnmappedFieldPolicy::Skip;
        for f in &mut deferred_only.fields {
            f.deferred = false;
        }
        let transformer = BatchTransformer::new(&deferred_only, None, self.extensions.as_deref());
        let sink = self.destination.sink()?.clone();
        let source = self.source.source()?.clone();
        let cp = self.store.get_checkpoint(&self.job_id, stage).await?;
        let mut cursor = cp.cursor.clone();
        self.store.set_stage_state(&self.job_id, stage, "running").await?;
        while !is_exhausted(&cursor) {
            if let Some(end) = self.check_control().await? {
                return Ok(end);
            }
            let batch = source.read_batch(object, cursor.as_ref(), self.plan.options.batch_size).await?;
            counters.read += batch.records.len() as u64;
            counters.batches += 1;
            let ids: Vec<String> = batch.records.iter().map(|r| r.id.clone()).collect();
            let dest: BTreeMap<String, String> = self.store.lookup_dest_ids(&self.job_id, object, &ids).await?.into_iter().collect();
            let transformed = transformer.transform(&batch.records, Some((&self.store, &self.job_id)), false).await?;
            let mut outcomes = vec![];
            for (rec, t) in batch.records.iter().zip(transformed) {
                let Some(dest_id) = dest.get(&rec.id) else { continue }; // not written in pass 1
                if t.fields.is_empty() {
                    continue;
                }
                counters.attempted += 1;
                if !t.errors.is_empty() {
                    counters.add_outcome(OutcomeKind::Failed, 1);
                    outcomes.push((rec.id.clone(), OutcomeKind::Failed, Some(format!("second pass: {}", t.errors.join("; ")))));
                    continue;
                }
                match sink.update_fields(&om.target_object, dest_id, t.fields).await {
                    Ok(()) => counters.add_outcome(OutcomeKind::Transformed, 1),
                    Err(e) => {
                        counters.add_outcome(OutcomeKind::Failed, 1);
                        outcomes.push((rec.id.clone(), OutcomeKind::Failed, Some(format!("second pass: {}", e.message))));
                    }
                }
            }
            cursor = Some(batch.next_cursor.clone().unwrap_or_else(|| json!({ EXHAUSTED: true })));
            self.store
                .commit_batch(BatchCommit {
                    job_id: &self.job_id,
                    stage,
                    source_object: object,
                    next_cursor: cursor.clone(),
                    sink_token: None,
                    id_maps: vec![],
                    outcomes,
                    exceptions: vec![],
                    counters: &counters,
                    stage_state: "running",
                })
                .await?;
            *commits += 1;
            maybe_crash("after_commit", *commits);
        }
        self.store.set_stage_state(&self.job_id, stage, "done").await?;
        Ok(RunEnd::Completed)
    }
}
