//! Self-building connector factory.
//!
//! `OpenAPI document → CapabilityManifest + DeclarativeConnectorSpec → static validation →
//! live contract tests → registry`. The document is treated strictly as data: descriptions are
//! carried as inert text and scanned for text addressed to AI agents, which is reported, never
//! obeyed. No part of a document can set hosts, credentials, approvals or policy.

pub mod contract;
pub mod openapi;
pub mod validate;

pub use contract::run_contract_tests;
pub use openapi::{GenerationResult, generate_from_openapi, parse_document};
pub use validate::{parse_spec, validate_spec};
