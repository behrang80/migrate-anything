//! Connector SDK.
//!
//! Connectors implement small, composable traits. A connector that can only be read implements
//! [`RecordSource`]; one that can be written implements [`RecordSink`]; key lookups are a separate
//! [`RecordLookup`] capability. All connectors integrate through [`ConnectorHandle`], so the
//! universal core never contains system-specific logic.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use ma_types::{
    Batch, CapabilityManifest, Cursor, FieldDef, FieldType, MaResult, ObjectDef, Record, SystemIdentity, SystemSchema, Value, WriteMode,
};

pub mod inference;

/// Identity, capabilities, connectivity and schema discovery.
#[async_trait]
pub trait Connector: Send + Sync {
    fn identify(&self) -> SystemIdentity;
    fn capabilities(&self) -> CapabilityManifest;
    /// Validate connectivity and credentials without reading business data.
    async fn check_connection(&self) -> MaResult<()>;
    async fn discover_schema(&self) -> MaResult<SystemSchema>;
    /// Version of the connector implementation (builtin version or spec version).
    fn connector_version(&self) -> String;
    /// Whether the connector accepts objects/fields not present in its discovered schema
    /// (e.g. file outputs that are created on write).
    fn creates_objects_on_write(&self) -> bool {
        false
    }
}

/// Cursor-based streaming reads. The cursor returned with each batch is the resume point.
#[async_trait]
pub trait RecordSource: Send + Sync {
    async fn read_batch(&self, object: &str, cursor: Option<&Cursor>, limit: usize) -> MaResult<Batch>;
    /// Exact count if cheaply available.
    async fn count(&self, object: &str) -> MaResult<Option<u64>>;
}

/// Description of one destination object for a write session.
#[derive(Debug, Clone)]
pub struct SinkObjectSpec {
    pub object: String,
    /// Destination fields in output order.
    pub fields: Vec<FieldDef>,
    /// Field holding the external (source) key; required for upserts.
    pub key_field: Option<String>,
    pub mode: WriteMode,
}

#[derive(Debug, Clone)]
pub struct WriteRequest {
    pub source_id: String,
    /// Fully transformed destination fields. Missing keys are not written.
    pub fields: BTreeMap<String, Value>,
}

/// Result for one record of a batch: the destination identifier or a record-level error.
pub type WriteResult = MaResult<String>;

#[async_trait]
pub trait RecordSink: Send + Sync {
    /// Begin (or resume) writing an object. `resume_token` is the value previously returned by
    /// [`RecordSink::durable_token`]; sinks that can roll back to it (files) must do so.
    async fn prepare(&self, spec: &SinkObjectSpec, resume_token: Option<&Cursor>) -> MaResult<()>;
    /// Write a batch. `Err` means nothing in the batch was written (safe to retry); `Ok` carries
    /// one result per request, in order.
    async fn write_batch(&self, object: &str, records: Vec<WriteRequest>) -> MaResult<Vec<WriteResult>>;
    /// Make previous writes durable and return a position token to store with the checkpoint.
    async fn durable_token(&self, object: &str) -> MaResult<Option<Cursor>>;
    /// Update fields of an already written record (second pass for cyclic references).
    async fn update_fields(&self, object: &str, dest_id: &str, fields: BTreeMap<String, Value>) -> MaResult<()> {
        let _ = (object, dest_id, fields);
        Err(ma_types::MaError::unsupported("this destination does not support updates"))
    }
    async fn finish(&self, object: &str) -> MaResult<()>;
    /// True if re-writing an already written batch cannot create duplicates.
    fn idempotent_rewrites(&self, mode: WriteMode, has_key: bool) -> bool;
    fn supports_updates(&self) -> bool {
        false
    }
}

/// Look up destination records by an external key.
#[async_trait]
pub trait RecordLookup: Send + Sync {
    async fn lookup_by_key(&self, object: &str, key_field: &str, key: &str) -> MaResult<Option<Record>>;
}

/// All capabilities of a constructed connector instance.
#[derive(Clone)]
pub struct ConnectorHandle {
    pub connector: Arc<dyn Connector>,
    pub source: Option<Arc<dyn RecordSource>>,
    pub sink: Option<Arc<dyn RecordSink>>,
    pub lookup: Option<Arc<dyn RecordLookup>>,
}

impl ConnectorHandle {
    pub fn source(&self) -> MaResult<&Arc<dyn RecordSource>> {
        self.source.as_ref().ok_or_else(|| ma_types::MaError::unsupported("connector cannot be read"))
    }
    pub fn sink(&self) -> MaResult<&Arc<dyn RecordSink>> {
        self.sink.as_ref().ok_or_else(|| ma_types::MaError::unsupported("connector cannot be written"))
    }
}

/// Convenience: build an object definition for writing from mapping targets when the
/// destination does not have one yet.
pub fn object_def_from_fields(name: &str, fields: Vec<(String, FieldType)>) -> ObjectDef {
    ObjectDef {
        name: name.to_string(),
        fields: fields.into_iter().map(|(n, t)| FieldDef::new(n, t)).collect(),
        primary_key: vec![],
        unique_keys: vec![],
        estimated_count: None,
        readable: true,
        writable: true,
    }
}
