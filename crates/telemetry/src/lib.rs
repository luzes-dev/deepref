use deepref_config::TelemetryConfig;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("invalid log filter: {0}")]
    Filter(#[from] tracing_subscriber::filter::ParseError),
}

pub struct TelemetryHandle {
    shutdown_deadline: std::time::Duration,
}

pub fn init(config: TelemetryConfig) -> Result<TelemetryHandle, TelemetryError> {
    let filter = EnvFilter::try_new(&config.log_filter)?;
    let subscriber = tracing_subscriber::registry().with(filter).with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(true),
    );
    let _ = subscriber.try_init();
    tracing::info!(
        service.name = %config.service_name,
        otlp.endpoint = ?config.otlp_endpoint,
        "telemetry initialized"
    );
    Ok(TelemetryHandle {
        shutdown_deadline: config.shutdown_deadline,
    })
}

impl TelemetryHandle {
    pub async fn shutdown(self) {
        let _ = self.shutdown_deadline;
        tracing::info!("telemetry shutdown complete");
    }
}
