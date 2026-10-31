//! Map adapter observations to application-owned, closed metric dimensions.
use deepref_integrations::jetstream::{ClaimOutcome, TransportObservation, TransportObserver};
use deepref_telemetry::transport::{TransportClaim, TransportMetrics};

pub(crate) struct RuntimeTransportObserver {
    metrics: TransportMetrics,
    readiness: tokio::sync::watch::Sender<
        Option<deepref_application::operational_health::TransportReadiness>,
    >,
}

impl RuntimeTransportObserver {
    pub(crate) fn readiness(
        &self,
    ) -> tokio::sync::watch::Receiver<
        Option<deepref_application::operational_health::TransportReadiness>,
    > {
        self.readiness.subscribe()
    }
}

impl Default for RuntimeTransportObserver {
    fn default() -> Self {
        let metrics = TransportMetrics::default();
        metrics.readiness(false);
        Self {
            metrics,
            readiness: tokio::sync::watch::channel(None).0,
        }
    }
}

impl TransportObserver for RuntimeTransportObserver {
    fn observe(&self, observation: TransportObservation) {
        match observation {
            TransportObservation::Readiness { ready } => {
                self.metrics.readiness(ready);
                self.readiness.send_replace(Some(
                    deepref_application::operational_health::TransportReadiness {
                        ready,
                        observed_at: chrono::Utc::now(),
                    },
                ));
            }
            TransportObservation::Visibility {
                pool,
                pending,
                in_flight,
                redeliveries,
            } => self
                .metrics
                .visibility(pool, pending, in_flight as u64, redeliveries as u64),
            TransportObservation::Claim {
                pool,
                outcome,
                elapsed,
            } => self.metrics.claim(
                pool,
                match outcome {
                    ClaimOutcome::Claimed => TransportClaim::Claimed,
                    ClaimOutcome::Noop => TransportClaim::Noop,
                    ClaimOutcome::Retry => TransportClaim::Retry,
                    ClaimOutcome::Failed => TransportClaim::Failed,
                },
                elapsed,
            ),
            TransportObservation::ConsumeLatency { elapsed } => self.metrics.delivery_age(elapsed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_watch_exists_without_an_otlp_exporter_and_retains_observation_time() {
        let observer = RuntimeTransportObserver::default();
        let watch = observer.readiness();
        assert!(watch.borrow().is_none());
        observer.observe(TransportObservation::Readiness { ready: true });
        let first = *watch.borrow();
        assert!(first.is_some_and(|value| value.ready));
        observer.observe(TransportObservation::Visibility {
            pool: deepref_domain::WorkloadClass::Documents,
            pending: 2,
            in_flight: 1,
            redeliveries: 0,
        });
        assert_eq!(
            *watch.borrow(),
            first,
            "other metrics cannot refresh readiness evidence"
        );
        observer.observe(TransportObservation::Readiness { ready: false });
        assert!(watch.borrow().is_some_and(|value| !value.ready));
    }
}
