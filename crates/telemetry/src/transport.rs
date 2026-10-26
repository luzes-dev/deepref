//! JetStream instrument vocabulary without an SDK dependency.

use deepref_domain::WorkloadClass;
use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Gauge, Histogram, Meter},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
pub enum TransportClaim {
    Claimed,
    Noop,
    Retry,
    Failed,
}

#[derive(Clone)]
pub struct TransportMetrics {
    ready: Gauge<u64>,
    ready_observed: Gauge<f64>,
    pending: Gauge<u64>,
    in_flight: Gauge<u64>,
    redeliveries: Gauge<u64>,
    visibility_observed: Gauge<f64>,
    claims: Counter<u64>,
    claim_duration: Histogram<f64>,
    delivery_age: Histogram<f64>,
}

impl Default for TransportMetrics {
    fn default() -> Self {
        Self::new(&global::meter("deepref.transport"))
    }
}

fn timestamp() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |time| time.as_secs_f64())
}

impl TransportMetrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            ready: meter.u64_gauge("deepref.transport.ready").build(),
            ready_observed: meter
                .f64_gauge("deepref.transport.readiness.last_observed")
                .with_unit("s")
                .build(),
            pending: meter.u64_gauge("deepref.transport.pending").build(),
            in_flight: meter.u64_gauge("deepref.transport.in_flight").build(),
            redeliveries: meter
                .u64_gauge("deepref.transport.redeliveries")
                .with_description(
                    "Current broker consumer redelivery count, not a cumulative local counter",
                )
                .build(),
            visibility_observed: meter
                .f64_gauge("deepref.transport.visibility.last_observed")
                .with_unit("s")
                .build(),
            claims: meter.u64_counter("deepref.transport.claims").build(),
            claim_duration: meter
                .f64_histogram("deepref.transport.claim.duration")
                .with_unit("s")
                .build(),
            delivery_age: meter
                .f64_histogram("deepref.transport.delivery.age")
                .with_description(
                    "Envelope age at intake, including DB/outbox residence; not handler duration",
                )
                .with_unit("s")
                .build(),
        }
    }
    pub fn readiness(&self, ready: bool) {
        self.ready.record(u64::from(ready), &[]);
        self.ready_observed.record(timestamp(), &[]);
    }
    pub fn visibility(&self, pool: WorkloadClass, pending: u64, in_flight: u64, redeliveries: u64) {
        let attributes = [KeyValue::new("pool", pool.as_str())];
        self.pending.record(pending, &attributes);
        self.in_flight.record(in_flight, &attributes);
        self.redeliveries.record(redeliveries, &attributes);
        self.visibility_observed.record(timestamp(), &attributes);
    }
    pub fn claim(&self, pool: WorkloadClass, outcome: TransportClaim, elapsed: Duration) {
        let attributes = [
            KeyValue::new("pool", pool.as_str()),
            KeyValue::new(
                "outcome",
                match outcome {
                    TransportClaim::Claimed => "claimed",
                    TransportClaim::Noop => "noop",
                    TransportClaim::Retry => "retry",
                    TransportClaim::Failed => "failed",
                },
            ),
        ];
        self.claims.add(1, &attributes);
        self.claim_duration
            .record(elapsed.as_secs_f64(), &attributes);
    }
    pub fn delivery_age(&self, elapsed: Duration) {
        self.delivery_age.record(elapsed.as_secs_f64(), &[]);
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
    fn broker_failure_changes_readiness_without_erasing_last_observed_visibility() {
        let exporter = InMemoryMetricExporterBuilder::new()
            .with_temporality(Temporality::Cumulative)
            .build();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let metrics = TransportMetrics::new(&provider.meter("transport-test"));
        metrics.readiness(true);
        metrics.visibility(WorkloadClass::Projection, 7, 2, 1);
        provider.force_flush().unwrap();
        exporter.reset();
        metrics.readiness(false);
        provider.force_flush().unwrap();
        let resources = exporter.get_finished_metrics().unwrap();
        let mut pending = None;
        let mut ready = None;
        let mut visibility_time = None;
        let mut ready_time = None;
        for metric in resources
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
        {
            match metric.data() {
                AggregatedMetrics::U64(MetricData::Gauge(gauge)) => {
                    for point in gauge.data_points() {
                        match metric.name() {
                            "deepref.transport.pending" => {
                                pending = Some(point.value());
                                assert_eq!(
                                    point.attributes().next().unwrap().value.to_string(),
                                    "projection"
                                );
                            }
                            "deepref.transport.ready" => ready = Some(point.value()),
                            _ => {}
                        }
                    }
                }
                AggregatedMetrics::F64(MetricData::Gauge(gauge)) => {
                    for point in gauge.data_points() {
                        match metric.name() {
                            "deepref.transport.visibility.last_observed" => {
                                visibility_time = Some(point.value())
                            }
                            "deepref.transport.readiness.last_observed" => {
                                ready_time = Some(point.value())
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        assert_eq!(pending, Some(7));
        assert_eq!(ready, Some(0));
        assert!(visibility_time.unwrap() > 0.0 && visibility_time <= ready_time);
        provider.shutdown().unwrap();
    }
}
