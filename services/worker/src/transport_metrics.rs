//! Map adapter observations to application-owned, closed metric dimensions.
use deepref_integrations::jetstream::{ClaimOutcome, TransportObservation, TransportObserver};
use deepref_telemetry::transport::{TransportClaim, TransportMetrics};

pub(crate) struct RuntimeTransportObserver(TransportMetrics);

impl Default for RuntimeTransportObserver {
    fn default() -> Self {
        let metrics = TransportMetrics::default();
        metrics.readiness(false);
        Self(metrics)
    }
}

impl TransportObserver for RuntimeTransportObserver {
    fn observe(&self, observation: TransportObservation) {
        match observation {
            TransportObservation::Readiness { ready } => self.0.readiness(ready),
            TransportObservation::Visibility {
                pool,
                pending,
                in_flight,
                redeliveries,
            } => self
                .0
                .visibility(pool, pending, in_flight as u64, redeliveries as u64),
            TransportObservation::Claim {
                pool,
                outcome,
                elapsed,
            } => self.0.claim(
                pool,
                match outcome {
                    ClaimOutcome::Claimed => TransportClaim::Claimed,
                    ClaimOutcome::Noop => TransportClaim::Noop,
                    ClaimOutcome::Retry => TransportClaim::Retry,
                    ClaimOutcome::Failed => TransportClaim::Failed,
                },
                elapsed,
            ),
            TransportObservation::ConsumeLatency { elapsed } => self.0.delivery_age(elapsed),
        }
    }
}
