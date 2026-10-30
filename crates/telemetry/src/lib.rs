pub mod metrics;
pub mod operational;
pub mod project_count_cache;
pub mod transport;

use std::time::Duration;

use deepref_config::TelemetryConfig;
use opentelemetry::{
    global,
    trace::{TraceContextExt, TracerProvider as _},
};
use opentelemetry_otlp::{
    MetricExporter, MetricExporterBuilder, NoExporterBuilderSet, SpanExporter, WithExportConfig,
};
use opentelemetry_sdk::{Resource, metrics::SdkMeterProvider, trace::SdkTracerProvider};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("invalid log filter: {0}")]
    Filter(#[from] tracing_subscriber::filter::ParseError),
    #[error("OTLP exporter could not be built: {0}")]
    Otlp(String),
    #[error("telemetry subscriber could not be installed: {0}")]
    Subscriber(String),
}

pub struct TelemetryHandle {
    shutdown_deadline: Duration,
    provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

fn build_otlp_provider(
    endpoint: &str,
    resource: Resource,
) -> Result<SdkTracerProvider, TelemetryError> {
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|error| TelemetryError::Otlp(error.to_string()))?;
    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build())
}

fn build_otlp_metric_exporter(
    endpoint: &str,
    builder: MetricExporterBuilder<NoExporterBuilderSet>,
) -> Result<MetricExporter, TelemetryError> {
    builder
        // Last-good gauge retention must not depend on an ambient exporter
        // preference: delta collection discards synchronous gauge samples.
        .with_temporality(opentelemetry_sdk::metrics::Temporality::Cumulative)
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| TelemetryError::Otlp(error.to_string()))
}

fn build_otlp_meter_provider(
    endpoint: &str,
    resource: Resource,
) -> Result<SdkMeterProvider, TelemetryError> {
    let exporter = build_otlp_metric_exporter(endpoint, MetricExporter::builder())?;
    Ok(SdkMeterProvider::builder()
        .with_periodic_exporter(exporter)
        .with_resource(resource)
        .build())
}

pub fn init(config: TelemetryConfig) -> Result<TelemetryHandle, TelemetryError> {
    let filter = EnvFilter::try_new(&config.log_filter)?;
    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .with_attributes([
            opentelemetry::KeyValue::new("service.version", config.build_sha.clone()),
            opentelemetry::KeyValue::new("service.instance.id", uuid::Uuid::new_v4().to_string()),
        ])
        .build();
    let meter_provider = config
        .otlp_endpoint
        .as_deref()
        .map(|endpoint| build_otlp_meter_provider(endpoint, resource.clone()))
        .transpose()?;
    let provider = config
        .otlp_endpoint
        .as_deref()
        .map(|endpoint| build_otlp_provider(endpoint, resource.clone()))
        .transpose()?;
    match provider.as_ref() {
        Some(provider) => {
            let tracer = provider.tracer("deepref");
            let subscriber = tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .with_current_span(true),
                )
                .with(tracing_opentelemetry::layer().with_tracer(tracer));
            subscriber
                .try_init()
                .map_err(|error| TelemetryError::Subscriber(error.to_string()))?;
            global::set_tracer_provider(provider.clone());
        }
        None => {
            let subscriber = tracing_subscriber::registry().with(filter).with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true),
            );
            subscriber
                .try_init()
                .map_err(|error| TelemetryError::Subscriber(error.to_string()))?;
        }
    }
    if let Some(provider) = meter_provider.as_ref() {
        global::set_meter_provider(provider.clone());
    }
    tracing::info!(service.name = %config.service_name, service.version = %config.build_sha, otlp.enabled = config.otlp_endpoint.is_some(), "telemetry initialized");
    Ok(TelemetryHandle {
        shutdown_deadline: config.shutdown_deadline,
        provider,
        meter_provider,
    })
}

pub fn current_trace_id() -> Option<String> {
    let context = tracing::Span::current().context();
    let span = context.span();
    let span_context = span.span_context();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
}

impl TelemetryHandle {
    pub async fn shutdown(self) {
        let started = std::time::Instant::now();
        if let Some(provider) = self.meter_provider
            && let Err(error) = provider.shutdown_with_timeout(self.shutdown_deadline)
        {
            tracing::warn!(%error, "metric exporter shutdown failed");
        }
        if let Some(provider) = self.provider {
            let remaining = self.shutdown_deadline.saturating_sub(started.elapsed());
            if let Err(error) = provider.shutdown_with_timeout(remaining) {
                tracing::warn!(%error, "trace exporter shutdown failed");
            }
        }
        tracing::info!("telemetry shutdown complete");
    }
}

#[cfg(all(test, feature = "operations-fixtures"))]
mod otlp_capture;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn explicit_delta_builder_cannot_discard_last_good_metric_samples() {
        use opentelemetry_sdk::metrics::exporter::PushMetricExporter;

        let endpoint = "http://127.0.0.1:4317";
        let builder = MetricExporter::builder()
            .with_temporality(opentelemetry_sdk::metrics::Temporality::Delta);
        let delta = builder
            .clone()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .expect("construct delta-configured exporter");
        assert_eq!(
            delta.temporality(),
            opentelemetry_sdk::metrics::Temporality::Delta,
            "the supplied builder must actually select delta before policy is applied"
        );
        let exporter = build_otlp_metric_exporter(endpoint, builder)
            .expect("construct policy-configured exporter");
        crate::metrics::tests::assert_last_good_retention(exporter.temporality());
    }

    #[test]
    fn invalid_metric_endpoint_is_rejected_during_construction() {
        assert!(build_otlp_meter_provider("not a URI", Resource::builder().build()).is_err());
    }

    #[test]
    fn invalid_otlp_endpoint_is_rejected_during_construction() {
        assert!(build_otlp_provider("not a URI", Resource::builder().build()).is_err());
    }
}
