//! Discovered schemas.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::value::FieldType;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FieldDef {
    pub name: String,
    pub field_type: FieldType,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub required: bool,
    /// The field is not part of the system's standard model (custom attribute).
    #[serde(default)]
    pub custom: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Allowed values when the system declares an enumeration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    /// Server-generated (serial ids, timestamps): not writable.
    #[serde(default)]
    pub read_only: bool,
    /// Native type name as reported by the system (e.g. `numeric(12,2)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_type: Option<String>,
}

impl FieldDef {
    pub fn new(name: impl Into<String>, field_type: FieldType) -> Self {
        FieldDef {
            name: name.into(),
            field_type,
            nullable: true,
            required: false,
            custom: false,
            description: None,
            enum_values: vec![],
            read_only: false,
            native_type: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Relationship {
    pub from_object: String,
    pub from_field: String,
    pub to_object: String,
    pub to_field: String,
    /// `documented` (FK constraint / spec) or `inferred` (naming heuristic).
    pub evidence: crate::capability::Evidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObjectDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    /// Identity / primary key field(s).
    pub primary_key: Vec<String>,
    /// Natural/unique keys usable for idempotent upserts.
    #[serde(default)]
    pub unique_keys: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_count: Option<u64>,
    #[serde(default)]
    pub readable: bool,
    #[serde(default)]
    pub writable: bool,
}

impl ObjectDef {
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct SystemSchema {
    pub objects: Vec<ObjectDef>,
    pub relationships: Vec<Relationship>,
}

impl SystemSchema {
    pub fn object(&self, name: &str) -> Option<&ObjectDef> {
        self.objects.iter().find(|o| o.name == name)
    }
}
