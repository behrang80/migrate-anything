//! Job state.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Running => "running",
            JobStatus::Paused => "paused",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
    pub fn is_terminal(self) -> bool {
        matches!(self, JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled)
    }
}

impl std::str::FromStr for JobStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "pending" => JobStatus::Pending,
            "running" => JobStatus::Running,
            "paused" => JobStatus::Paused,
            "completed" => JobStatus::Completed,
            "failed" => JobStatus::Failed,
            "cancelled" => JobStatus::Cancelled,
            o => return Err(format!("unknown job status {o}")),
        })
    }
}

/// Requested control action, polled by the worker between batches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobControl {
    None,
    Pause,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    /// Written; every mapped field was a direct copy or lossless type conversion.
    MigratedNatively,
    /// Written; at least one non-trivial transformation was applied.
    Transformed,
    /// Written, and some source data was preserved in the exception archive.
    WrittenWithArchive,
    /// Not written on purpose (filtered / excluded).
    SkippedIntentionally,
    /// Not written: contains values the destination cannot represent.
    Unsupported,
    /// Not written: error.
    Failed,
    /// Write may or may not have happened (non-idempotent destination, crash in flight).
    RequiresReconciliation,
}

impl OutcomeKind {
    pub fn is_written(self) -> bool {
        matches!(self, OutcomeKind::MigratedNatively | OutcomeKind::Transformed | OutcomeKind::WrittenWithArchive)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            OutcomeKind::MigratedNatively => "migrated_natively",
            OutcomeKind::Transformed => "transformed",
            OutcomeKind::WrittenWithArchive => "written_with_archive",
            OutcomeKind::SkippedIntentionally => "skipped_intentionally",
            OutcomeKind::Unsupported => "unsupported",
            OutcomeKind::Failed => "failed",
            OutcomeKind::RequiresReconciliation => "requires_reconciliation",
        }
    }
}

impl std::str::FromStr for OutcomeKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "migrated_natively" => OutcomeKind::MigratedNatively,
            "transformed" => OutcomeKind::Transformed,
            "written_with_archive" => OutcomeKind::WrittenWithArchive,
            "skipped_intentionally" => OutcomeKind::SkippedIntentionally,
            "unsupported" => OutcomeKind::Unsupported,
            "failed" => OutcomeKind::Failed,
            "requires_reconciliation" => OutcomeKind::RequiresReconciliation,
            o => return Err(format!("unknown outcome {o}")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct StageCounters {
    pub read: u64,
    pub attempted: u64,
    pub outcomes: BTreeMap<String, u64>,
    pub archived_values: u64,
    pub retries: u64,
    pub batches: u64,
}

impl StageCounters {
    pub fn add_outcome(&mut self, kind: OutcomeKind, n: u64) {
        *self.outcomes.entry(kind.as_str().to_string()).or_default() += n;
    }
    pub fn outcome(&self, kind: OutcomeKind) -> u64 {
        self.outcomes.get(kind.as_str()).copied().unwrap_or(0)
    }
    pub fn written(&self) -> u64 {
        self.outcome(OutcomeKind::MigratedNatively) + self.outcome(OutcomeKind::Transformed) + self.outcome(OutcomeKind::WrittenWithArchive)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StageProgress {
    pub source_object: String,
    /// `pending`, `running`, `done`, `second_pass`.
    pub state: String,
    pub counters: StageCounters,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JobInfo {
    pub job_id: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub status: JobStatus,
    pub control: JobControl,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub worker_pid: Option<u32>,
    pub error: Option<String>,
    pub stages: Vec<StageProgress>,
    pub attempts: u32,
}
