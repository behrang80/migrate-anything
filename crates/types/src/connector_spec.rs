//! Declarative connector specifications.
//!
//! A spec selects behaviours that the trusted REST runtime already implements. It cannot contain
//! code, absolute URLs, hosts or secrets; the base URL and credentials come from the connection,
//! which is controlled by the operator.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::schema::{FieldDef, Relationship};

pub const CONNECTOR_SPEC_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeclarativeConnectorSpec {
    pub spec_version: u32,
    /// Lower-case `[a-z0-9-]`, 3..64 chars.
    pub connector_id: String,
    /// `MAJOR.MINOR.PATCH`.
    pub version: String,
    pub protocol: SpecProtocol,
    pub title: String,
    /// Version of the remote API the spec was derived from (e.g. OpenAPI `info.version`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    /// Path prefix appended to the connection base URL (e.g. `/api/v1`). Must start with `/`.
    #[serde(default)]
    pub base_path: String,
    pub authentication: AuthSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimitSpec>,
    pub resources: Vec<ResourceSpec>,
    pub provenance: SpecProvenance,
}

impl DeclarativeConnectorSpec {
    pub fn resource(&self, name: &str) -> Option<&ResourceSpec> {
        self.resources.iter().find(|r| r.name == name)
    }
    pub fn reference(&self) -> String {
        format!("{}@{}", self.connector_id, self.version)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpecProtocol {
    Rest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthSpec {
    None,
    /// `Authorization: Bearer <credential>`.
    Bearer,
    /// Credential sent in a named header.
    ApiKeyHeader {
        header: String,
    },
    /// Credential is `user:password`, sent as HTTP Basic.
    Basic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimitSpec {
    /// Client-side ceiling; 0 means unlimited.
    pub max_requests_per_second: u32,
    /// Honour `Retry-After` on 429/503.
    #[serde(default = "default_true")]
    pub respect_retry_after: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceSpec {
    pub name: String,
    pub identity_field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<ListOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<WriteOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<WriteOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub get: Option<ItemOperation>,
    /// Find records by an external key via a list filter parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookup: Option<LookupOperation>,
    #[serde(default)]
    pub fields: Vec<FieldDef>,
    #[serde(default)]
    pub relationships: Vec<Relationship>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListOperation {
    /// Relative path, e.g. `/customers`.
    pub path: String,
    /// Dotted path to the array in the response body; empty when the body is the array.
    #[serde(default)]
    pub records_path: String,
    pub pagination: Pagination,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Pagination {
    None,
    Cursor {
        /// Query parameter carrying the cursor.
        cursor_param: String,
        /// Dotted path in the response body to the next cursor (null/absent = end).
        next_cursor_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit_param: Option<String>,
        page_size: u32,
    },
    Offset {
        offset_param: String,
        limit_param: String,
        page_size: u32,
    },
    Page {
        page_param: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size_param: Option<String>,
        page_size: u32,
        first_page: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteOperation {
    pub method: HttpMethod,
    /// Relative path; may contain `{id}` for item operations.
    pub path: String,
    /// Dotted path to the created/updated record's identity in the response.
    #[serde(default)]
    pub response_id_path: String,
    /// The API is documented as idempotent for this operation (e.g. PUT by key).
    #[serde(default)]
    pub idempotent: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemOperation {
    /// Relative path containing `{id}`.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LookupOperation {
    /// Relative list path.
    pub path: String,
    /// Query parameter used to filter by the key value.
    pub query_param: String,
    /// Field in returned records holding the key.
    pub key_field: String,
    #[serde(default)]
    pub records_path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecProvenance {
    /// `connector-factory/openapi@<version>` or `manual`.
    pub generated_by: String,
    pub generated_at: String,
    /// SHA-256 of the source document the spec was derived from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_document_sha256: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Lifecycle of a generated connector in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorStatus {
    /// Generated and statically validated.
    Draft,
    /// Contract tests passed.
    Tested,
    /// Approved by an operator for use in executable plans.
    Active,
    /// Rejected or superseded.
    Disabled,
}

impl std::fmt::Display for ConnectorStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ConnectorStatus::Draft => "draft",
            ConnectorStatus::Tested => "tested",
            ConnectorStatus::Active => "active",
            ConnectorStatus::Disabled => "disabled",
        })
    }
}

impl std::str::FromStr for ConnectorStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "draft" => ConnectorStatus::Draft,
            "tested" => ConnectorStatus::Tested,
            "active" => ConnectorStatus::Active,
            "disabled" => ConnectorStatus::Disabled,
            o => return Err(format!("unknown connector status {o}")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContractTestResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}
