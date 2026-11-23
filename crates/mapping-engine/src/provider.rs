//! AI provider abstraction.
//!
//! Providers receive **schema metadata only** (object/field names, types, descriptions, enum
//! values) — never records. A provider that sends data outside the machine must report
//! `sends_data_externally() == true`; the application refuses to use such a provider unless the
//! operator enabled it in configuration. Suggestions from any AI provider are always marked
//! `NeedsReview`; they cannot become executable without review.
//!
//! v1 ships the local deterministic [`HeuristicProvider`]. When Migrate Anything is driven over
//! MCP, the calling agent acts as the AI layer: it reviews `migrate_mapping_suggest` output and
//! submits an edited mapping to `migrate_plan_create`, where the same validation applies.

use async_trait::async_trait;
use ma_types::{MaResult, MappingSuggestion, SystemSchema};

#[derive(Debug, Clone)]
pub struct MappingRequest {
    pub source: SystemSchema,
    pub destination: SystemSchema,
    /// Explicit `(source object, destination object)` pairs; empty = provider pairs objects.
    pub pairs: Vec<(String, String)>,
}

#[async_trait]
pub trait AiProvider: Send + Sync {
    fn name(&self) -> &str;
    fn sends_data_externally(&self) -> bool;
    async fn suggest(&self, request: &MappingRequest) -> MaResult<Vec<MappingSuggestion>>;
}

/// Local, deterministic provider backed by [`crate::suggest`].
pub struct HeuristicProvider;

#[async_trait]
impl AiProvider for HeuristicProvider {
    fn name(&self) -> &str {
        "heuristic"
    }
    fn sends_data_externally(&self) -> bool {
        false
    }
    async fn suggest(&self, request: &MappingRequest) -> MaResult<Vec<MappingSuggestion>> {
        let set = crate::suggest::suggest_mapping(
            &request.source,
            &request.destination,
            &request.pairs,
            &crate::suggest::SuggestOptions::default(),
        );
        Ok(set.suggestions)
    }
}
