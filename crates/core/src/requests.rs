//! Typed request/response contracts shared by the CLI and MCP adapters.

use ma_types::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DiscoveryResult {
    pub connection: String,
    pub identity: SystemIdentity,
    pub manifest: CapabilityManifest,
    pub schema: SystemSchema,
    pub connector: ConnectorRef,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FieldProfile {
    pub field: String,
    pub declared_type: FieldType,
    pub nulls: usize,
    pub missing: usize,
    pub invalid: usize,
    pub distinct_in_sample: usize,
    pub max_length: usize,
    /// Only populated when explicitly requested.
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ObjectProfile {
    pub object: String,
    pub sampled: usize,
    pub fields: Vec<FieldProfile>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SchemaAnalysis {
    pub discovery: DiscoveryResult,
    pub profiles: Vec<ObjectProfile>,
    pub examples_included: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocumentSource {
    /// Local OpenAPI file (JSON or YAML).
    File { path: String },
    /// Fetch over HTTP(S) using the named connection's network allowlist.
    Url { url: String, connection: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GenerateConnectorRequest {
    pub source: DocumentSource,
    /// `[a-z0-9-]{3,64}`
    pub connector_id: String,
    /// Attach the generated connector to this `rest` connection.
    #[serde(default)]
    pub attach_to_connection: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GenerateConnectorResponse {
    pub connector_ref: String,
    pub status: ConnectorStatus,
    pub reused_existing: bool,
    pub spec: DeclarativeConnectorSpec,
    pub manifest: CapabilityManifest,
    pub notes: Vec<String>,
    pub suspicious_text: Vec<String>,
    pub validation_errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ConnectorTestResponse {
    pub connector_ref: String,
    pub passed: bool,
    pub status: ConnectorStatus,
    pub results: Vec<ContractTestResult>,
    pub manifest: CapabilityManifest,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RegistryEntry {
    pub reference: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub generated: bool,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlanRequest {
    pub source: String,
    pub destination: String,
    /// Explicit mapping. When absent, the heuristic suggestion is used (fields below the
    /// confidence threshold stay `needs_review` and block approval).
    #[serde(default)]
    pub mapping: Option<MappingSpec>,
    /// Object pairs `(source, destination)` used when no mapping is given.
    #[serde(default)]
    pub pairs: Vec<(String, String)>,
    #[serde(default)]
    pub options: Option<ExecutionOptions>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PlanSummary {
    pub plan_id: String,
    pub plan_hash: String,
    pub source: String,
    pub destination: String,
    pub execution_order: Vec<String>,
    pub operations: Vec<Operation>,
    pub blocking_risks: Vec<String>,
    pub warnings: Vec<String>,
    pub needs_review: Vec<String>,
    pub approvable: bool,
    pub markdown: String,
}

impl PlanSummary {
    pub fn from_plan(plan: &MigrationPlan, hash: &str, markdown: String) -> Self {
        let by =
            |s: Severity| plan.risks.iter().filter(|r| r.severity == s).map(|r| format!("[{}] {}", r.code, r.message)).collect::<Vec<_>>();
        PlanSummary {
            plan_id: plan.plan_id.clone(),
            plan_hash: hash.into(),
            source: plan.source.connection.clone(),
            destination: plan.destination.connection.clone(),
            execution_order: plan.execution_order.clone(),
            operations: plan.operations.clone(),
            blocking_risks: by(Severity::Blocking),
            warnings: by(Severity::Warning),
            needs_review: plan.unresolved_mappings.clone(),
            approvable: !plan.has_blocking_risks(),
            markdown,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PlanPreview {
    pub summary: PlanSummary,
    pub dry_run: Option<DryRunReport>,
    pub approval: ApprovalStatus,
    pub plan: MigrationPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnMode {
    /// Start a detached worker process (survives the caller).
    Detached,
    /// Do not spawn; the caller runs `App::run_worker` itself.
    Inline,
}
