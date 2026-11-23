//! Approval records. Created only by the operator-facing approval workflow (interactive CLI),
//! never by the MCP server.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::capability::SystemIdentity;
use crate::connection::Environment;
use crate::plan::{ConnectorRef, Operation};

/// Everything an approval is bound to. Signed as canonical JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalClaims {
    pub approval_id: String,
    pub workspace: String,
    pub plan_id: String,
    pub plan_hash: String,
    pub source_identity: SystemIdentity,
    pub destination_identity: SystemIdentity,
    pub source_connector: ConnectorRef,
    pub destination_connector: ConnectorRef,
    pub permitted_operations: Vec<Operation>,
    /// Destructive operations require this flag in addition to being listed.
    pub destructive_authorized: bool,
    pub operator: String,
    /// How the operator was verified (e.g. `interactive-tty-challenge`).
    pub verification_method: String,
    pub target_environment: Environment,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRecord {
    pub claims: ApprovalClaims,
    /// Hex HMAC-SHA256 over the canonical JSON of `claims`.
    pub signature: String,
    #[serde(default)]
    pub revoked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    /// No approval and no request.
    NotRequested,
    /// An agent asked for approval; waiting for the operator.
    Pending,
    Approved,
    Expired,
    Revoked,
    /// Approval exists but no longer matches the plan.
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalStatus {
    pub plan_id: String,
    pub plan_hash: String,
    pub state: ApprovalState,
    pub detail: String,
    /// What the operator must run to approve.
    pub operator_command: String,
}
