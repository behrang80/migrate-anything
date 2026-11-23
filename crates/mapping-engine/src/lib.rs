//! Mapping: advisory suggestions (heuristic or AI) and the deterministic runtime that executes
//! accepted [`ma_types::MappingSpec`]s.

pub mod provider;
pub mod runtime;
pub mod suggest;

pub use provider::{AiProvider, HeuristicProvider, MappingRequest};
pub use runtime::{EvalContext, ExtensionRunner, RefLookup, TransformedRecord, cast_value, is_native, transform_record};
pub use suggest::{SuggestOptions, SuggestionSet, suggest_mapping};
