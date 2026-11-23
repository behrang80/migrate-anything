//! System identity and capability manifests.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How strongly a capability claim is supported. Callers must never treat `Documented` or
/// `Inferred` as `Verified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// Exercised successfully against the live system.
    Verified,
    /// Stated by the system's own documentation or metadata (OpenAPI, catalog, FK constraint).
    Documented,
    /// Derived by heuristics (naming conventions, sampled values).
    Inferred,
    /// Known not to be supported.
    Unsupported,
    /// Not determined.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SystemIdentity {
    /// Connector family, e.g. `files`, `postgres`, `rest`.
    pub kind: String,
    /// Human-facing name (OpenAPI title, database name, directory).
    pub name: String,
    /// Product or API version if known.
    pub version: Option<String>,
    /// Stable locator without secrets (redacted URL, path).
    pub locator: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityEntry {
    pub evidence: Evidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Versioned description of what a system can do, with evidence for every claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityManifest {
    pub manifest_version: u32,
    pub identity: SystemIdentity,
    /// Well-known capability keys: `read`, `write`, `update`, `delete`, `schema_discovery`,
    /// `pagination`, `incremental_read`, `transactions`, `upsert`, `lookup_by_key`,
    /// `attachments`, `rate_limits`, `authentication`, `history`.
    pub capabilities: BTreeMap<String, CapabilityEntry>,
    /// Per-object capabilities.
    #[serde(default)]
    pub objects: BTreeMap<String, BTreeMap<String, CapabilityEntry>>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

impl CapabilityManifest {
    pub fn new(identity: SystemIdentity) -> Self {
        CapabilityManifest { manifest_version: 1, identity, capabilities: BTreeMap::new(), objects: BTreeMap::new(), limitations: vec![] }
    }

    pub fn set(&mut self, key: &str, evidence: Evidence, detail: Option<String>) -> &mut Self {
        self.capabilities.insert(key.to_string(), CapabilityEntry { evidence, detail });
        self
    }

    pub fn set_object(&mut self, object: &str, key: &str, evidence: Evidence, detail: Option<String>) {
        self.objects.entry(object.to_string()).or_default().insert(key.to_string(), CapabilityEntry { evidence, detail });
    }

    pub fn evidence(&self, key: &str) -> Evidence {
        self.capabilities.get(key).map(|c| c.evidence).unwrap_or(Evidence::Unknown)
    }

    /// True only for verified or documented support.
    pub fn supports(&self, key: &str) -> bool {
        matches!(self.evidence(key), Evidence::Verified | Evidence::Documented)
    }
}
