//! Records, provenance and batches.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::value::Value;

/// Where a record came from. Never fabricated: fields are `None` when the source did not say.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct Provenance {
    pub system: String,
    pub object: String,
    pub source_id: String,
    pub extracted_at: Option<DateTime<Utc>>,
    pub source_created_at: Option<String>,
    pub source_updated_at: Option<String>,
}

/// A reference to binary content belonging to a record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentRef {
    pub name: String,
    pub content_type: Option<String>,
    pub size_bytes: Option<u64>,
    /// Location understood by the source connector (path, URL, blob key).
    pub locator: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Record {
    pub object: String,
    /// Source identifier (stringified primary key / identity field).
    pub id: String,
    pub fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentRef>,
    #[serde(default)]
    pub provenance: Provenance,
}

impl Record {
    pub fn new(object: impl Into<String>, id: impl Into<String>) -> Self {
        let object = object.into();
        let id = id.into();
        Record {
            provenance: Provenance { object: object.clone(), source_id: id.clone(), ..Default::default() },
            object,
            id,
            fields: BTreeMap::new(),
            attachments: Vec::new(),
        }
    }

    pub fn with(mut self, k: impl Into<String>, v: Value) -> Self {
        self.fields.insert(k.into(), v);
        self
    }

    /// Resolve a dotted path. Returns `None` when the field is missing (as opposed to null).
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        let mut parts = path.split('.');
        let first = parts.next()?;
        let rest: Vec<&str> = parts.collect();
        self.fields.get(first)?.get_path(&rest)
    }

    pub fn to_plain_json(&self) -> serde_json::Value {
        serde_json::Value::Object(self.fields.iter().map(|(k, v)| (k.clone(), v.to_plain_json())).collect())
    }
}

/// Opaque connector-defined position in a stream (byte offset, keyset, page token...).
pub type Cursor = serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Batch {
    pub records: Vec<Record>,
    /// `None` means the stream is exhausted after this batch.
    pub next_cursor: Option<Cursor>,
}
