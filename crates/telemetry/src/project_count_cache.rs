//! Observe cache operations and cancellation without project or tenant labels.
use deepref_application::project_count_cache::{
    ArticleCountSnapshot, CountCacheFuture, ProjectCountCache, ProjectCountKey,
};
use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Histogram, Meter},
};
use std::{sync::Arc, time::Instant};

#[derive(Clone)]
pub struct ProjectCountMetrics {
    operations: Counter<u64>,
    entries: Counter<u64>,
    duration: Histogram<f64>,
}
impl Default for ProjectCountMetrics {
    fn default() -> Self {
        Self::new(&global::meter("deepref.project_count_cache"))
    }
}
impl ProjectCountMetrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            operations: meter
                .u64_counter("deepref.project_count_cache.operations")
                .build(),
            entries: meter
                .u64_counter("deepref.project_count_cache.entries")
                .with_description("Hit and miss entries returned by completed reads")
                .build(),
            duration: meter
                .f64_histogram("deepref.project_count_cache.duration")
                .with_unit("s")
                .build(),
        }
    }
}

struct Operation<'a> {
    metrics: &'a ProjectCountMetrics,
    kind: &'static str,
    outcome: &'static str,
    started: Instant,
}
impl<'a> Operation<'a> {
    fn new(metrics: &'a ProjectCountMetrics, kind: &'static str) -> Self {
        Self {
            metrics,
            kind,
            outcome: "cancelled",
            started: Instant::now(),
        }
    }
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        let attributes = [
            KeyValue::new("operation", self.kind),
            KeyValue::new("outcome", self.outcome),
        ];
        self.metrics.operations.add(1, &attributes);
        self.metrics
            .duration
            .record(self.started.elapsed().as_secs_f64(), &attributes);
    }
}

pub struct ObservedProjectCountCache {
    inner: Arc<dyn ProjectCountCache>,
    metrics: ProjectCountMetrics,
}
impl ObservedProjectCountCache {
    pub fn new(inner: Arc<dyn ProjectCountCache>, metrics: ProjectCountMetrics) -> Self {
        Self { inner, metrics }
    }
}
impl ProjectCountCache for ObservedProjectCountCache {
    fn get<'a>(
        &'a self,
        keys: &'a [ProjectCountKey],
    ) -> CountCacheFuture<'a, Vec<Option<ArticleCountSnapshot>>> {
        Box::pin(async move {
            let mut operation = Operation::new(&self.metrics, "get");
            let result = self.inner.get(keys).await;
            operation.outcome = if result.is_ok() {
                "completed"
            } else {
                "unavailable"
            };
            if let Ok(values) = &result {
                let hits = values.iter().filter(|value| value.is_some()).count() as u64;
                self.metrics
                    .entries
                    .add(hits, &[KeyValue::new("outcome", "hit")]);
                self.metrics.entries.add(
                    values.len() as u64 - hits,
                    &[KeyValue::new("outcome", "miss")],
                );
            }
            result
        })
    }
    fn put<'a>(&'a self, values: &'a [ArticleCountSnapshot]) -> CountCacheFuture<'a, ()> {
        Box::pin(async move {
            let mut operation = Operation::new(&self.metrics, "put");
            let result = self.inner.put(values).await;
            operation.outcome = if result.is_ok() {
                "completed"
            } else {
                "unavailable"
            };
            result
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
mod tests {
    use super::*;
    use deepref_application::project_count_cache::CountCacheUnavailable;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::{
        InMemoryMetricExporterBuilder, PeriodicReader, SdkMeterProvider, Temporality,
        data::{AggregatedMetrics, MetricData},
    };
    struct Stalled;
    impl ProjectCountCache for Stalled {
        fn get<'a>(
            &'a self,
            _: &'a [ProjectCountKey],
        ) -> CountCacheFuture<'a, Vec<Option<ArticleCountSnapshot>>> {
            Box::pin(std::future::pending())
        }
        fn put<'a>(&'a self, _: &'a [ArticleCountSnapshot]) -> CountCacheFuture<'a, ()> {
            Box::pin(async { Err(CountCacheUnavailable) })
        }
    }
    #[tokio::test]
    async fn cancellation_and_failure_are_exported_without_claiming_successful_fallback() {
        let exporter = InMemoryMetricExporterBuilder::new()
            .with_temporality(Temporality::Cumulative)
            .build();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let cache = ObservedProjectCountCache::new(
            Arc::new(Stalled),
            ProjectCountMetrics::new(&provider.meter("cache-test")),
        );
        let mut pending = cache.get(&[]);
        assert!(
            pending
                .as_mut()
                .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
                .is_pending()
        );
        drop(pending);
        assert!(cache.put(&[]).await.is_err());
        provider.force_flush().unwrap();
        let resources = exporter.get_finished_metrics().unwrap();
        let mut outcomes = Vec::new();
        for metric in resources
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
        {
            if metric.name() == "deepref.project_count_cache.operations"
                && let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data()
            {
                for point in sum.data_points() {
                    assert_eq!(point.value(), 1);
                    assert_eq!(point.attributes().count(), 2);
                    outcomes.push(
                        point
                            .attributes()
                            .find(|attribute| attribute.key.as_str() == "outcome")
                            .unwrap()
                            .value
                            .to_string(),
                    );
                }
            }
        }
        outcomes.sort();
        assert_eq!(outcomes, vec!["cancelled", "unavailable"]);
        provider.shutdown().unwrap();
    }
}
