//! Compiled, review-specific semantics behind a small durable-execution seam.
//!
//! Callers select a checked-in definition and schedule a typed subject. The
//! implementation owns workflow topology, asset identity, run manifests, and
//! node fingerprints. Provider calls remain in `deepref-ai`; PostgreSQL and
//! worker adapters live outside this crate.

mod definition;
mod hash;
mod manifest;
mod task;
mod types;

pub use definition::{CompiledReviewDefinition, CompiledReviewIdentity, ReviewCatalog};
pub use hash::ReviewHash;
pub use manifest::{
    AcceptedArtifactInput, ReviewManifestInput, ReviewModelIdentity, ReviewRunManifest,
    ReviewRuntimeIdentity, fingerprint_node,
};
pub use task::DefinedAiTask;
pub use types::{
    CalibrationBundleId, ReviewBlockCode, ReviewDefinitionKey, ReviewError, ReviewFuture,
    ReviewOrigin, ReviewRunId, ReviewRunSnapshot, ReviewRunState, ReviewScheduler, ReviewSubject,
    ScheduleReviewRun,
};
