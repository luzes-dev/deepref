//! Compiled, review-specific semantics behind a small durable-execution seam.
//!
//! Callers select a checked-in definition and schedule a typed subject. The
//! implementation owns workflow topology, asset identity, run manifests, and
//! node fingerprints. Provider calls remain in `deepref-ai`; PostgreSQL and
//! worker adapters live outside this crate.

mod definition;
mod execution;
mod hash;
mod identity;
mod manifest;
#[doc(hidden)]
pub mod memory;
mod task;
mod types;
#[doc(hidden)]
pub mod worker;

/// The fingerprint logic that `build.rs` runs, compiled here only so that its
/// unit tests run with the crate's test harness.
#[cfg(test)]
#[path = "../build_support/fingerprint.rs"]
mod build_support;

pub(crate) use definition::{CompiledReviewDefinition, ReviewCatalog};
pub(crate) use hash::ReviewHash;
#[doc(hidden)]
pub use identity::SEMANTIC_DEPENDENCIES;
pub use identity::{
    IdentityComparison, IdentityComponent, SEMANTIC_IDENTITY_SCHEME, SemanticIdentity,
};
pub(crate) use task::DefinedAiTask;
pub use types::{
    CalibrationBundleId, ReviewBlockCode, ReviewDefinitionKey, ReviewError, ReviewFuture,
    ReviewOrigin, ReviewRunId, ReviewRunSnapshot, ReviewRunState, ReviewScheduler, ReviewSubject,
    ScheduleReviewRun,
};
