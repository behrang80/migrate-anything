//! Immutable migration plans and dry-run results.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::capability::SystemIdentity;
use crate::connection::Environment;
use crate::mapping::MappingSpec;
use crate::schema::SystemSchema;

pub const PLAN_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectorRef {
    pub kind: String,
    /// `builtin` connectors carry the platform version; generated ones their spec version.
    pub id: String,
    pub version: String,
    pub generated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanEndpoint {
    pub connection: String,
    pub identity: SystemIdentity,
    pub connector: ConnectorRef,
    pub environment: Environment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Read,
    Insert,
    Upsert,
    /// Second-pass updates for deferred (cyclic) references.
    Update,
    /// Destructive: requires separate authorization.
    TruncateDestination,
}

impl Operation {
    pub fn is_destructive(self) -> bool {
        matches!(self, Operation::TruncateDestination)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    /// Blocks approval until resolved.
    Blocking,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Risk {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionOptions {
    pub batch_size: usize,
    pub max_retries: u32,
    pub retry_base_ms: u64,
    /// Read-ahead depth (bounded channel capacity in batches).
    pub prefetch_batches: usize,
    /// Number of records re-read and compared field-by-field during verification.
    pub verify_sample_size: usize,
}

impl Default for ExecutionOptions {
    fn default() -> Self {
        ExecutionOptions { batch_size: 500, max_retries: 5, retry_base_ms: 200, prefetch_batches: 2, verify_sample_size: 50 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeferredReference {
    pub source_object: String,
    pub target_field: String,
    pub references: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MigrationPlan {
    pub plan_format_version: u32,
    pub plan_id: String,
    pub workspace: String,
    pub created_at: DateTime<Utc>,
    pub source: PlanEndpoint,
    pub destination: PlanEndpoint,
    /// Discovered schemas (selected objects only).
    pub source_schema: SystemSchema,
    pub destination_schema: SystemSchema,
    pub mapping: MappingSpec,
    /// Source objects in dependency order.
    pub execution_order: Vec<String>,
    pub deferred_references: Vec<DeferredReference>,
    pub estimated_records: BTreeMap<String, Option<u64>>,
    pub operations: Vec<Operation>,
    pub unresolved_mappings: Vec<String>,
    pub risks: Vec<Risk>,
    pub verification_strategy: Vec<String>,
    pub recovery_strategy: Vec<String>,
    pub approval_requirements: Vec<String>,
    pub options: ExecutionOptions,
    /// Registered wasm extensions the mapping uses, as `name -> sha256`.
    #[serde(default)]
    pub extensions: BTreeMap<String, String>,
}

impl MigrationPlan {
    /// SHA-256 over the canonical JSON form. Any change to the plan changes the hash.
    pub fn compute_hash(&self) -> String {
        crate::hash::canonical_sha256(self)
    }

    pub fn has_blocking_risks(&self) -> bool {
        self.risks.iter().any(|r| r.severity == Severity::Blocking)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct DryRunObjectResult {
    pub source_object: String,
    pub target_object: String,
    pub sampled: u64,
    pub transform_ok: u64,
    pub transform_failed: u64,
    pub missing_required: BTreeMap<String, u64>,
    pub invalid_values: BTreeMap<String, u64>,
    pub unresolved_reference_checks: u64,
    pub duplicate_keys_in_sample: u64,
    pub archived_fields: Vec<String>,
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DryRunReport {
    pub plan_hash: String,
    pub wrote_to_destination: bool,
    pub objects: Vec<DryRunObjectResult>,
    pub issues: Vec<Risk>,
    pub passed: bool,
}
