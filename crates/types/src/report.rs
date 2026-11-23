//! Verification results and the final migration report.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FinalStatus {
    CompletedAndVerified,
    CompletedWithExceptions,
    PartiallyCompleted,
    Failed,
    Cancelled,
    /// Job finished but has not been verified yet.
    NotVerified,
}

impl std::fmt::Display for FinalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FinalStatus::CompletedAndVerified => "Completed and verified",
            FinalStatus::CompletedWithExceptions => "Completed with exceptions",
            FinalStatus::PartiallyCompleted => "Partially completed",
            FinalStatus::Failed => "Failed",
            FinalStatus::Cancelled => "Cancelled",
            FinalStatus::NotVerified => "Not verified",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CheckResult {
    pub name: String,
    pub object: Option<String>,
    pub passed: bool,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct ObjectVerification {
    pub source_object: String,
    pub target_object: String,
    pub source_count: u64,
    pub eligible: u64,
    pub written: u64,
    pub destination_count: Option<u64>,
    pub keys_verified: u64,
    pub keys_missing: u64,
    pub duplicate_keys: u64,
    pub references_checked: u64,
    pub references_broken: u64,
    pub sampled: u64,
    pub sample_mismatches: u64,
    pub checksum_match: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationResult {
    pub job_id: String,
    pub verified_at: DateTime<Utc>,
    pub objects: Vec<ObjectVerification>,
    pub checks: Vec<CheckResult>,
    pub discrepancies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MigrationReport {
    pub source_system: String,
    pub destination_system: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub job_id: String,
    pub execution_start: Option<DateTime<Utc>>,
    pub execution_end: Option<DateTime<Utc>>,
    pub records_discovered: u64,
    pub records_eligible: u64,
    pub records_attempted: u64,
    pub records_written: u64,
    pub records_verified: u64,
    pub records_skipped: u64,
    pub records_failed: u64,
    pub records_requiring_review: u64,
    pub outcomes: BTreeMap<String, u64>,
    pub relationships_checked: u64,
    pub relationships_preserved: u64,
    pub attachments_migrated: u64,
    pub archived_values: u64,
    pub unsupported_fields: Vec<String>,
    pub unresolved_discrepancies: Vec<String>,
    pub final_status: FinalStatus,
    pub verified: bool,
}

impl MigrationReport {
    pub fn to_text(&self) -> String {
        let ts = |t: &Option<DateTime<Utc>>| t.map(|t| t.to_rfc3339()).unwrap_or_else(|| "-".into());
        let mut s = String::new();
        s.push_str("MIGRATION REPORT\n\n");
        s.push_str(&format!("Source system:            {}\n", self.source_system));
        s.push_str(&format!("Destination system:       {}\n", self.destination_system));
        s.push_str(&format!("Migration plan:           {} (sha256 {})\n", self.plan_id, &self.plan_hash[..16.min(self.plan_hash.len())]));
        s.push_str(&format!("Job:                      {}\n", self.job_id));
        s.push_str(&format!("Execution start:          {}\n", ts(&self.execution_start)));
        s.push_str(&format!("Execution end:            {}\n\n", ts(&self.execution_end)));
        s.push_str(&format!("Records discovered:       {}\n", self.records_discovered));
        s.push_str(&format!("Records eligible:         {}\n", self.records_eligible));
        s.push_str(&format!("Records attempted:        {}\n", self.records_attempted));
        s.push_str(&format!("Records written:          {}\n", self.records_written));
        s.push_str(&format!(
            "Records verified:         {}\n",
            if self.verified { self.records_verified.to_string() } else { "not verified".into() }
        ));
        s.push_str(&format!("Records skipped:          {}\n", self.records_skipped));
        s.push_str(&format!("Records failed:           {}\n", self.records_failed));
        s.push_str(&format!("Records requiring review: {}\n\n", self.records_requiring_review));
        s.push_str(&format!("Relationships preserved:  {} of {} checked\n", self.relationships_preserved, self.relationships_checked));
        s.push_str(&format!("Attachments migrated:     {}\n", self.attachments_migrated));
        s.push_str(&format!("Archived source values:   {}\n", self.archived_values));
        s.push_str(&format!(
            "Unsupported fields:       {}\n",
            if self.unsupported_fields.is_empty() { "none".into() } else { self.unsupported_fields.join(", ") }
        ));
        s.push_str(&format!("Unresolved discrepancies: {}\n", self.unresolved_discrepancies.len()));
        for d in &self.unresolved_discrepancies {
            s.push_str(&format!("  - {d}\n"));
        }
        s.push_str("\nOutcomes:\n");
        for (k, v) in &self.outcomes {
            s.push_str(&format!("  {k}: {v}\n"));
        }
        s.push_str(&format!("\nFinal status: {}\n", self.final_status));
        s
    }
}
