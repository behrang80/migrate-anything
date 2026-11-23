//! Mapping specifications (deterministic) and mapping suggestions (advisory).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::value::{FieldType, Value};

/// A deterministic expression evaluated against one source record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expr {
    /// Source field by dotted path. Missing stays missing.
    Field {
        path: String,
    },
    /// A literal value.
    Const {
        value: Value,
    },
    /// Convert to a type; unconvertible input becomes `Invalid` (never silently dropped).
    Cast {
        input: Box<Expr>,
        to: FieldType,
    },
    /// Join the text of several expressions; missing/null parts are skipped.
    Concat {
        parts: Vec<Expr>,
        #[serde(default)]
        separator: String,
    },
    /// Take one part of a split string.
    Split {
        input: Box<Expr>,
        separator: String,
        index: usize,
    },
    /// Map source values (by text) to destination values.
    EnumMap {
        input: Box<Expr>,
        map: BTreeMap<String, Value>,
        #[serde(default)]
        on_unmapped: Unmapped,
    },
    /// First expression that is present and not null.
    Coalesce {
        inputs: Vec<Expr>,
    },
    /// Use `value` when the input is missing or null.
    Default {
        input: Box<Expr>,
        value: Value,
    },
    Trim {
        input: Box<Expr>,
    },
    Lower {
        input: Box<Expr>,
    },
    Upper {
        input: Box<Expr>,
    },
    /// Parse text into a date/datetime using a chrono format string.
    ParseDate {
        input: Box<Expr>,
        format: String,
        #[serde(default)]
        datetime: bool,
    },
    /// Exact decimal multiply by 10^pow (currency major/minor units).
    ScaleDecimal {
        input: Box<Expr>,
        pow: i32,
    },
    /// Exact linear unit conversion `value * factor` (factor as decimal text).
    UnitConvert {
        input: Box<Expr>,
        factor: String,
    },
    /// Resolve a reference to another migrated object through the job's id map.
    Ref {
        input: Box<Expr>,
        object: String,
    },
    /// Run a registered sandboxed WebAssembly extension.
    Extension {
        input: Box<Expr>,
        name: String,
    },
    /// Build a nested object from named expressions.
    Object {
        fields: BTreeMap<String, Expr>,
    },
}

impl Expr {
    pub fn field(path: impl Into<String>) -> Expr {
        Expr::Field { path: path.into() }
    }

    /// Source field paths read by this expression.
    pub fn source_paths(&self, out: &mut Vec<String>) {
        match self {
            Expr::Field { path } => out.push(path.clone()),
            Expr::Const { .. } => {}
            Expr::Concat { parts, .. } | Expr::Coalesce { inputs: parts } => parts.iter().for_each(|p| p.source_paths(out)),
            Expr::Object { fields } => fields.values().for_each(|p| p.source_paths(out)),
            Expr::Cast { input, .. }
            | Expr::Split { input, .. }
            | Expr::EnumMap { input, .. }
            | Expr::Default { input, .. }
            | Expr::Trim { input }
            | Expr::Lower { input }
            | Expr::Upper { input }
            | Expr::ParseDate { input, .. }
            | Expr::ScaleDecimal { input, .. }
            | Expr::UnitConvert { input, .. }
            | Expr::Ref { input, .. }
            | Expr::Extension { input, .. } => input.source_paths(out),
        }
    }

    /// Objects referenced through `Ref`.
    pub fn referenced_objects(&self, out: &mut Vec<String>) {
        match self {
            Expr::Ref { input, object } => {
                out.push(object.clone());
                input.referenced_objects(out)
            }
            Expr::Concat { parts, .. } | Expr::Coalesce { inputs: parts } => parts.iter().for_each(|p| p.referenced_objects(out)),
            Expr::Object { fields } => fields.values().for_each(|p| p.referenced_objects(out)),
            Expr::Cast { input, .. }
            | Expr::Split { input, .. }
            | Expr::EnumMap { input, .. }
            | Expr::Default { input, .. }
            | Expr::Trim { input }
            | Expr::Lower { input }
            | Expr::Upper { input }
            | Expr::ParseDate { input, .. }
            | Expr::ScaleDecimal { input, .. }
            | Expr::UnitConvert { input, .. }
            | Expr::Extension { input, .. } => input.referenced_objects(out),
            Expr::Field { .. } | Expr::Const { .. } => {}
        }
    }

    pub fn extensions(&self, out: &mut Vec<String>) {
        if let Expr::Extension { name, input } = self {
            out.push(name.clone());
            input.extensions(out);
        }
        match self {
            Expr::Concat { parts, .. } | Expr::Coalesce { inputs: parts } => parts.iter().for_each(|p| p.extensions(out)),
            Expr::Object { fields } => fields.values().for_each(|p| p.extensions(out)),
            Expr::Cast { input, .. }
            | Expr::Split { input, .. }
            | Expr::EnumMap { input, .. }
            | Expr::Default { input, .. }
            | Expr::Trim { input }
            | Expr::Lower { input }
            | Expr::Upper { input }
            | Expr::ParseDate { input, .. }
            | Expr::ScaleDecimal { input, .. }
            | Expr::UnitConvert { input, .. }
            | Expr::Ref { input, .. } => input.extensions(out),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Unmapped {
    /// Record fails with a clear error (default: no silent change of meaning).
    #[default]
    Fail,
    /// Write null and archive the original value.
    NullAndArchive,
    /// Keep the original value unchanged.
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// Accepted by deterministic rules (exact match) or by a reviewer.
    Accepted,
    /// Must be reviewed before the plan can be approved.
    NeedsReview,
    /// Explicitly rejected; ignored at execution.
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FieldMapping {
    /// Destination field name.
    pub target: String,
    pub expr: Expr,
    /// 0.0..=1.0
    pub confidence: f32,
    pub status: ReviewStatus,
    #[serde(default)]
    pub explanation: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Set by the planner when the reference participates in a cycle: written in a second pass.
    #[serde(default)]
    pub deferred: bool,
}

impl FieldMapping {
    pub fn accepted(target: impl Into<String>, expr: Expr) -> Self {
        FieldMapping {
            target: target.into(),
            expr,
            confidence: 1.0,
            status: ReviewStatus::Accepted,
            explanation: "specified explicitly".into(),
            evidence: vec![],
            deferred: false,
        }
    }
}

/// What happens to source fields that no mapping reads. Nothing is silently discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum UnmappedFieldPolicy {
    /// Preserve in the structured exception archive (default).
    #[default]
    Archive,
    /// Intentionally skipped; counted and reported.
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    /// Plain insert. Not idempotent: a crash between write and checkpoint may duplicate.
    Insert,
    /// Insert or update by the destination external key. Idempotent.
    #[default]
    Upsert,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObjectMapping {
    pub source_object: String,
    pub target_object: String,
    /// Destination field that stores the source identifier (external id). Strongly recommended:
    /// it makes writes idempotent and verification possible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_key_field: Option<String>,
    pub fields: Vec<FieldMapping>,
    #[serde(default)]
    pub unmapped_fields: UnmappedFieldPolicy,
    /// Source fields explicitly skipped (reported as "skipped intentionally").
    #[serde(default)]
    pub skip_fields: Vec<String>,
    #[serde(default)]
    pub write_mode: WriteMode,
}

impl ObjectMapping {
    pub fn referenced_objects(&self) -> Vec<String> {
        let mut out = vec![];
        for f in &self.fields {
            if f.status != ReviewStatus::Rejected {
                f.expr.referenced_objects(&mut out);
            }
        }
        out.sort();
        out.dedup();
        out
    }

    pub fn source_paths(&self) -> Vec<String> {
        let mut out = vec![];
        for f in &self.fields {
            if f.status != ReviewStatus::Rejected {
                f.expr.source_paths(&mut out);
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct MappingSpec {
    pub mapping_version: u32,
    pub objects: Vec<ObjectMapping>,
}

impl MappingSpec {
    pub fn object(&self, source_object: &str) -> Option<&ObjectMapping> {
        self.objects.iter().find(|o| o.source_object == source_object)
    }
}

/// An advisory suggestion produced by the heuristic matcher or an AI provider. It is never
/// executed directly; it is converted into a [`FieldMapping`] whose status reflects confidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MappingSuggestion {
    pub source_object: String,
    pub target_object: String,
    pub source_field: Option<String>,
    pub target_field: String,
    pub expr: Expr,
    pub confidence: f32,
    pub explanation: String,
    pub evidence: Vec<String>,
    /// `heuristic` or the AI provider name.
    pub suggested_by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObjectPairing {
    pub source_object: String,
    pub target_object: String,
    pub confidence: f32,
    pub explanation: String,
}
