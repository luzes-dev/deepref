//! Compiled, review-specific semantics behind a small durable-execution seam.
//!
//! Callers select a checked-in definition and schedule a typed subject. The
//! implementation owns workflow topology, asset identity, run manifests, and
//! node fingerprints. Provider calls remain in `deepref-ai`; PostgreSQL and
//! worker adapters live outside this crate.

mod definition;
#[doc(hidden)]
pub mod execution;
mod hash;
mod manifest;
#[doc(hidden)]
pub mod memory;
mod task;
mod types;

pub(crate) use definition::{CompiledReviewDefinition, ReviewCatalog};
pub(crate) use hash::ReviewHash;
pub(crate) use task::DefinedAiTask;
pub use types::{
    CalibrationBundleId, ReviewBlockCode, ReviewDefinitionKey, ReviewError, ReviewFuture,
    ReviewOrigin, ReviewRunId, ReviewRunSnapshot, ReviewRunState, ReviewScheduler, ReviewSubject,
    ScheduleReviewRun,
};

/// Worker and persistence mechanics. HTTP and assistant callers must use the
/// typed scheduling interface re-exported at the crate root.
#[doc(hidden)]
pub mod internal {
    pub use crate::definition::{
        CompiledReviewDefinition, CompiledReviewIdentity, ReviewCatalog, ReviewTransitionSignal,
    };
    pub use crate::hash::ReviewHash;
    pub use crate::manifest::{
        AcceptedArtifactInput, ReviewManifestInput, ReviewModelIdentity, ReviewRunManifest,
        ReviewRuntimeIdentity, fingerprint_node,
    };
    pub use crate::task::DefinedAiTask;
}
