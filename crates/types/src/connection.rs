//! Connection configuration. Contains no secrets: credentials are referenced by name.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    /// Disposable target; contract-test writes are allowed here.
    Test,
    Staging,
    Production,
}

impl std::fmt::Display for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Environment::Test => "test",
            Environment::Staging => "staging",
            Environment::Production => "production",
        })
    }
}

impl std::str::FromStr for Environment {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "test" => Ok(Environment::Test),
            "staging" => Ok(Environment::Staging),
            "production" | "prod" => Ok(Environment::Production),
            other => Err(format!("unknown environment `{other}` (test|staging|production)")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectionConfig {
    pub name: String,
    /// Connector kind: `files`, `postgres`, `rest`.
    pub kind: String,
    pub environment: Environment,
    /// Kind-specific settings (see docs/connectors). Must not contain secrets.
    pub settings: serde_json::Value,
    /// Name of an encrypted credential in the workspace secret store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    /// Hosts this connection may contact (exact host names or IP literals).
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// For `rest`: the registered generated connector, `id@version`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_ref: Option<String>,
}
