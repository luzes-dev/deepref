//! Application-owned, fixed-cardinality metric instruments. Never use a
//! resource identifier, arbitrary URL, provider payload or error as a label.
use std::time::Duration;

use deepref_domain::WorkloadClass;
use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Gauge, Histogram, Meter},
};

#[derive(Debug, Clone, Copy)]
pub enum RouteClass {
    Health,
    Identity,
    Webhook,
    Billing,
    Account,
    Documents,
    Import,
    Assistant,
    Research,
    Other,
    Unmatched,
}

impl RouteClass {
    const fn label(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Identity => "identity",
            Self::Webhook => "webhook",
            Self::Billing => "billing",
            Self::Account => "account",
            Self::Documents => "documents",
            Self::Import => "import",
            Self::Assistant => "assistant",
            Self::Research => "research",
            Self::Other => "other",
            Self::Unmatched => "unmatched",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Other,
}

impl HttpMethod {
    const fn label(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Other => "OTHER",
        }
    }
}

#[derive(Clone)]
pub struct HttpMetrics {
    requests: Counter<u64>,
    duration: Histogram<f64>,
    schema_behind: Gauge<u64>,
}

impl Default for HttpMetrics {
    fn default() -> Self {
        Self::new(&global::meter("deepref.http"))
    }
}

impl HttpMetrics {
    pub fn global() -> &'static Self {
        static METRICS: std::sync::OnceLock<HttpMetrics> = std::sync::OnceLock::new();
        METRICS.get_or_init(Self::default)
    }

    pub fn new(meter: &Meter) -> Self {
        Self {
            requests: meter
                .u64_counter("deepref.http.requests")
                .with_description("Responses started, including SSE handshakes and rejections")
                .with_unit("{request}")
                .build(),
            duration: meter
                .f64_histogram("deepref.http.response_header.duration")
                .with_description("Time to response headers; SSE responses excluded")
                .with_unit("s")
                .with_boundaries(vec![
                    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                ])
                .build(),
            schema_behind: meter
                .u64_gauge("deepref.schema.behind")
                .with_description(
                    "Last readiness schema check: 1 when older than this binary; 0 otherwise",
                )
                .build(),
        }
    }

    pub fn response(
        &self,
        route: RouteClass,
        method: HttpMethod,
        status: u16,
        elapsed: Duration,
        sse: bool,
    ) {
        let status_class = match status / 100 {
            1 => "1xx",
            2 => "2xx",
            3 => "3xx",
            4 => "4xx",
            5 => "5xx",
            _ => "other",
        };
        let attributes = [
            KeyValue::new("route_class", route.label()),
            KeyValue::new("method", method.label()),
            KeyValue::new("status_class", status_class),
        ];
        self.requests.add(1, &attributes);
        if !sse {
            self.duration.record(elapsed.as_secs_f64(), &attributes);
        }
    }

    pub fn schema_version(&self, actual: i64, required: i64) {
        self.schema_behind.record(u64::from(actual < required), &[]);
    }
}

/// Fixed diagnostic families; each is a global database aggregate, not a tenant metric.
#[derive(Debug, Clone, Copy)]
pub enum Backlog {
    Outbox,
    AutonomyOutbox,
    Inbox,
    UsageSettlement,
    UsageExport,
    UsageUnemitted,
}

impl Backlog {
    const fn label(self) -> &'static str {
        match self {
            Self::Outbox => "outbox",
            Self::AutonomyOutbox => "autonomy_outbox",
            Self::Inbox => "inbox",
            Self::UsageSettlement => "usage_settlement",
            Self::UsageExport => "usage_export",
            Self::UsageUnemitted => "usage_unemitted",
        }
    }
}

#[derive(Clone)]
pub struct WorkerMetrics {
    queue_depth: Gauge<u64>,
    queue_age: Gauge<f64>,
    in_flight: Gauge<u64>,
    dead_depth: Gauge<u64>,
    capacity: Gauge<u64>,
    live_workers: Gauge<u64>,
    backlog: Gauge<u64>,
    backlog_age: Gauge<f64>,
    sample_up: Gauge<u64>,
    export_conflicts: Gauge<u64>,
    export_unattributed: Gauge<u64>,
    last_success: Gauge<f64>,
}

impl Default for WorkerMetrics {
    fn default() -> Self {
        Self::new(&global::meter("deepref.worker"))
    }
}

impl WorkerMetrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            queue_depth: meter
                .u64_gauge("deepref.worker.queue.depth")
                .with_description("Due queued and capability-wait jobs by pool")
                .build(),
            queue_age: meter
                .f64_gauge("deepref.worker.queue.oldest_due.age")
                .with_unit("s")
                .build(),
            in_flight: meter.u64_gauge("deepref.worker.in_flight").build(),
            dead_depth: meter
                .u64_gauge("deepref.worker.dead.depth")
                .with_description("Retained dead jobs requiring review")
                .build(),
            capacity: meter.u64_gauge("deepref.worker.capacity").build(),
            live_workers: meter
                .u64_gauge("deepref.worker.live")
                .with_description("Heartbeats seen within 60 seconds")
                .build(),
            backlog: meter.u64_gauge("deepref.backlog.depth").build(),
            backlog_age: meter
                .f64_gauge("deepref.backlog.oldest.age")
                .with_unit("s")
                .build(),
            sample_up: meter.u64_gauge("deepref.worker.metrics.sample.up").build(),
            export_conflicts: meter
                .u64_gauge("deepref.usage_export.identity_conflicts")
                .build(),
            export_unattributed: meter.u64_gauge("deepref.usage_export.unattributed").build(),
            last_success: meter
                .f64_gauge("deepref.worker.metrics.last_success")
                .with_unit("s")
                .with_description("Unix timestamp of the last complete database metric sample")
                .build(),
        }
    }

    pub fn pool(
        &self,
        pool: WorkloadClass,
        depth: u64,
        age: f64,
        in_flight: u64,
        dead: u64,
        capacity: u64,
    ) {
        let attributes = [KeyValue::new("pool", pool.as_str())];
        self.queue_depth.record(depth, &attributes);
        self.queue_age.record(age, &attributes);
        self.in_flight.record(in_flight, &attributes);
        self.dead_depth.record(dead, &attributes);
        self.capacity.record(capacity, &attributes);
    }

    pub fn live_workers(&self, count: u64) {
        self.live_workers.record(count, &[]);
    }

    pub fn backlog(&self, family: Backlog, depth: u64, age: f64) {
        let attributes = [KeyValue::new("family", family.label())];
        self.backlog.record(depth, &attributes);
        self.backlog_age.record(age, &attributes);
    }

    pub fn export_attention(&self, conflicting: u64, unattributed: u64) {
        self.export_conflicts.record(conflicting, &[]);
        self.export_unattributed.record(unattributed, &[]);
    }

    pub fn sample_succeeded(&self, unix_seconds: f64) {
        self.last_success.record(unix_seconds, &[]);
        self.sample_up.record(1, &[]);
    }

    /// Preserve last-known aggregates and timestamp. Never report a failed read as zero backlog.
    pub fn sample_failed(&self) {
        self.sample_up.record(0, &[]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub(crate) mod tests {
    use super::*;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::{
        InMemoryMetricExporterBuilder, PeriodicReader, SdkMeterProvider, Temporality,
        data::{AggregatedMetrics, MetricData},
    };

    #[test]
    fn failed_samples_do_not_turn_backlog_into_zero_or_advance_success_time() {
        assert_last_good_retention(Temporality::Cumulative);
    }

    pub(crate) fn assert_last_good_retention(temporality: Temporality) {
        let exporter = InMemoryMetricExporterBuilder::new()
            .with_temporality(temporality)
            .build();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let metrics = WorkerMetrics::new(&provider.meter("worker-test"));
        metrics.pool(WorkloadClass::Ai, 7, 120.0, 1, 2, 3);
        metrics.sample_succeeded(1000.0);
        provider.force_flush().unwrap();
        exporter.reset();
        // A collection with no new database sample must still carry the last
        // good counts and snapshot timestamp, before a failure is recorded.
        provider.force_flush().unwrap();
        let idle = exporter.get_finished_metrics().unwrap();
        assert!(
            idle.iter()
                .flat_map(|resource| resource.scope_metrics())
                .flat_map(|scope| scope.metrics())
                .any(|metric| metric.name() == "deepref.worker.queue.depth"),
            "an idle collection must retain the last good depth"
        );
        exporter.reset();
        metrics.sample_failed();
        provider.force_flush().unwrap();
        let mut saw_depth = false;
        let mut saw_failure = false;
        let mut saw_last_success = false;
        for metric in exporter
            .get_finished_metrics()
            .unwrap()
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
        {
            match metric.data() {
                AggregatedMetrics::U64(MetricData::Gauge(gauge))
                    if metric.name() == "deepref.worker.queue.depth" =>
                {
                    let values = gauge
                        .data_points()
                        .map(|point| point.value())
                        .collect::<Vec<_>>();
                    assert_eq!(values, vec![7]);
                    saw_depth = true;
                }
                AggregatedMetrics::U64(MetricData::Gauge(gauge))
                    if metric.name() == "deepref.worker.metrics.sample.up" =>
                {
                    assert_eq!(gauge.data_points().next().unwrap().value(), 0);
                    saw_failure = true;
                }
                AggregatedMetrics::F64(MetricData::Gauge(gauge))
                    if metric.name() == "deepref.worker.metrics.last_success" =>
                {
                    assert_eq!(gauge.data_points().next().unwrap().value(), 1000.0);
                    saw_last_success = true;
                }
                _ => {}
            }
        }
        assert!(saw_depth && saw_failure && saw_last_success);
        provider.shutdown().unwrap();
    }
}
