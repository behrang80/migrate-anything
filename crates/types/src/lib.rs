//! Shared, domain-neutral contracts used by every Migrate Anything layer.
//!
//! Nothing in this crate knows about a particular system, protocol or vendor.

pub mod approval;
pub mod capability;
pub mod connection;
pub mod connector_spec;
pub mod error;
pub mod hash;
pub mod job;
pub mod json_text;
pub mod mapping;
pub mod plan;
pub mod record;
pub mod report;
pub mod schema;
pub mod value;

pub use approval::*;
pub use capability::*;
pub use connection::*;
pub use connector_spec::*;
pub use error::*;
pub use job::*;
pub use mapping::*;
pub use plan::*;
pub use record::*;
pub use report::*;
pub use schema::*;
pub use value::*;
