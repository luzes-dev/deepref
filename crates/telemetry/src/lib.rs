use std::time::Duration;

use deepref_config::TelemetryConfig;
use opentelemetry::{
    global,
    trace::{TraceContextExt, TracerProvider as _},
};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::{Resource, trace::SdkTracerProvider};
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
}

fn build_otlp_provider(
    endpoint: &str,
    service_name: &str,
) -> Result<SdkTracerProvider, TelemetryError> {
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|error| TelemetryError::Otlp(error.to_string()))?;
    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_service_name(service_name.to_owned())
                .build(),
        )
        .build())
}

pub fn init(config: TelemetryConfig) -> Result<TelemetryHandle, TelemetryError> {
    let filter = EnvFilter::try_new(&config.log_filter)?;
    let provider = config
        .otlp_endpoint
        .as_deref()
        .map(|endpoint| build_otlp_provider(endpoint, &config.service_name))
        .transpose()?;
    match provider.clone() {
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
    tracing::info!(service.name = %config.service_name, otlp.enabled = config.otlp_endpoint.is_some(), "telemetry initialized");
    Ok(TelemetryHandle {
        shutdown_deadline: config.shutdown_deadline,
        provider,
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
        if let Some(provider) = self.provider {
            let _ = provider.force_flush();
            let _ = provider.shutdown_with_timeout(self.shutdown_deadline);
        }
        tracing::info!("telemetry shutdown complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_otlp_endpoint_is_rejected_during_construction() {
        assert!(build_otlp_provider("not a URI", "deepref").is_err());
    }
}
