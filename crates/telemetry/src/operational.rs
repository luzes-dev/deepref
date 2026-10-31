//! Fixed-cardinality bridges for optional operational infrastructure.
//! Resource IDs, payloads, connection URLs and driver errors never enter labels.

use std::time::Duration;

use deepref_application::{
    concurrency_mirror::{
        ConcurrencyMirrorObserver, MirrorOperation, MirrorOutcome, MirrorScopeKind,
    },
    rate_limits::{
        RateLimitBackend, RateLimitDecision, RateLimitObserver, RateLimitPurpose,
        RateLimitUnavailable,
    },
};
use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Histogram, Meter},
};

#[derive(Clone)]
pub struct OperationalMetrics {
    rate_decisions: Counter<u64>,
    rate_duration: Histogram<f64>,
    rate_fallbacks: Counter<u64>,
    mirror_operations: Counter<u64>,
    mirror_duration: Histogram<f64>,
}

impl Default for OperationalMetrics {
    fn default() -> Self {
        Self::new(&global::meter("deepref.operational"))
    }
}

impl OperationalMetrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            rate_decisions: meter.u64_counter("deepref.rate_limit.decisions").build(),
            rate_duration: meter
                .f64_histogram("deepref.rate_limit.duration")
                .with_unit("s")
                .build(),
            rate_fallbacks: meter.u64_counter("deepref.rate_limit.fallbacks").build(),
            mirror_operations: meter
                .u64_counter("deepref.concurrency_mirror.operations")
                .build(),
            mirror_duration: meter
                .f64_histogram("deepref.concurrency_mirror.duration")
                .with_unit("s")
                .build(),
        }
    }
}

impl RateLimitObserver for OperationalMetrics {
    fn decision(
        &self,
        purpose: RateLimitPurpose,
        backend: RateLimitBackend,
        outcome: Result<RateLimitDecision, RateLimitUnavailable>,
        elapsed: Duration,
    ) {
        let attributes = [
            KeyValue::new("purpose", purpose.as_str()),
            KeyValue::new(
                "backend",
                match backend {
                    RateLimitBackend::Postgres => "postgres",
                    RateLimitBackend::Valkey => "valkey",
                },
            ),
            KeyValue::new(
                "outcome",
                match outcome {
                    Ok(RateLimitDecision::Allowed) => "allowed",
                    Ok(RateLimitDecision::Limited) => "limited",
                    Err(_) => "unavailable",
                },
            ),
        ];
        self.rate_decisions.add(1, &attributes);
        self.rate_duration
            .record(elapsed.as_secs_f64(), &attributes);
    }

    fn fallback(&self, purpose: RateLimitPurpose) {
        self.rate_fallbacks
            .add(1, &[KeyValue::new("purpose", purpose.as_str())]);
    }
}

impl ConcurrencyMirrorObserver for OperationalMetrics {
    fn observe(
        &self,
        scope: MirrorScopeKind,
        operation: MirrorOperation,
        outcome: MirrorOutcome,
        elapsed: Duration,
    ) {
        let attributes = [
            KeyValue::new(
                "scope",
                match scope {
                    MirrorScopeKind::Workspace => "workspace",
                    MirrorScopeKind::Route => "route",
                },
            ),
            KeyValue::new(
                "operation",
                match operation {
                    MirrorOperation::Check => "check",
                    MirrorOperation::Hold => "hold",
                    MirrorOperation::Refresh => "refresh",
                    MirrorOperation::Release => "release",
                },
            ),
            KeyValue::new(
                "outcome",
                match outcome {
                    MirrorOutcome::Available => "available",
                    MirrorOutcome::Saturated => "saturated",
                    MirrorOutcome::Unavailable => "unavailable",
                },
            ),
        ];
        self.mirror_operations.add(1, &attributes);
        self.mirror_duration
            .record(elapsed.as_secs_f64(), &attributes);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
mod tests {
    use super::*;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::{
        InMemoryMetricExporterBuilder, PeriodicReader, SdkMeterProvider, Temporality,
        data::{AggregatedMetrics, MetricData},
    };

    #[test]
    fn real_exporter_emits_refusal_fallback_and_error_with_only_closed_dimensions() {
        let exporter = InMemoryMetricExporterBuilder::new()
            .with_temporality(Temporality::Cumulative)
            .build();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let metrics = OperationalMetrics::new(&provider.meter("operational-test"));
        metrics.decision(
            RateLimitPurpose::WorkflowWebhook,
            RateLimitBackend::Valkey,
            Err(RateLimitUnavailable),
            Duration::from_millis(100),
        );
        metrics.fallback(RateLimitPurpose::WorkflowWebhook);
        metrics.decision(
            RateLimitPurpose::WorkflowWebhook,
            RateLimitBackend::Postgres,
            Ok(RateLimitDecision::Limited),
            Duration::from_millis(2),
        );
        metrics.observe(
            MirrorScopeKind::Route,
            MirrorOperation::Hold,
            MirrorOutcome::Unavailable,
            Duration::from_millis(100),
        );
        provider.force_flush().unwrap();
        let resources = exporter.get_finished_metrics().unwrap();
        let mut decisions = 0;
        let mut fallback = 0;
        let mut mirror = 0;
        for metric in resources
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
        {
            if let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() {
                for point in sum.data_points() {
                    let labels = point
                        .attributes()
                        .map(|attribute| (attribute.key.as_str(), attribute.value.to_string()))
                        .collect::<Vec<_>>();
                    assert!(labels.iter().all(|(name, _)| matches!(
                        *name,
                        "purpose" | "backend" | "outcome" | "scope" | "operation"
                    )));
                    match metric.name() {
                        "deepref.rate_limit.decisions" => {
                            decisions += point.value();
                            assert!(
                                labels.contains(&("outcome", "unavailable".into()))
                                    || labels.contains(&("outcome", "limited".into()))
                            );
                        }
                        "deepref.rate_limit.fallbacks" => fallback += point.value(),
                        "deepref.concurrency_mirror.operations" => {
                            mirror += point.value();
                            assert!(labels.contains(&("scope", "route".into())));
                        }
                        _ => {}
                    }
                }
            }
        }
        assert_eq!((decisions, fallback, mirror), (2, 1, 1));
        provider.shutdown().unwrap();
    }
}
