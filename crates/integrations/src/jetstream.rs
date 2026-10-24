//! JetStream SDK boundary. PostgreSQL owns every consequential decision.
use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use async_nats::jetstream::{
    self,
    consumer::{self, PullConsumer},
    stream,
};
use deepref_application::work_delivery::EventEnvelope;
use deepref_config::{ExposeSecret, NatsConfig};
use deepref_domain::{WorkloadClass, events::MAX_DELIVERIES};
use futures::StreamExt;
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct JetStream {
    context: jetstream::Context,
    config: NatsConfig,
}

impl JetStream {
    /// Provision absent resources, validate existing resources exactly. A
    /// rolling deployment must never silently mutate retention or ACK policy.
    pub async fn connect(config: NatsConfig) -> Result<Self> {
        let mut options = async_nats::ConnectOptions::new()
            .connection_timeout(config.request_timeout)
            .request_timeout(Some(config.request_timeout));
        if let Some(token) = &config.token {
            options = options.token(token.expose_secret().to_owned());
        }
        let client = options
            .connect(&config.url)
            .await
            .context("connect to NATS")?;
        let context = jetstream::context::ContextBuilder::new()
            .timeout(config.request_timeout)
            .ack_timeout(config.request_timeout)
            .build(client);
        let adapter = Self { context, config };
        adapter.provision().await?;
        Ok(adapter)
    }

    fn stream_config(&self, dead: bool) -> stream::Config {
        stream::Config {
            name: if dead {
                format!("{}_DLQ", self.config.stream)
            } else {
                self.config.stream.clone()
            },
            subjects: if dead {
                vec![format!("{}.dead.>", self.config.subject_prefix)]
            } else {
                vec![
                    format!("{}.jobs.*", self.config.subject_prefix),
                    format!("{}.events", self.config.subject_prefix),
                ]
            },
            retention: stream::RetentionPolicy::Limits,
            storage: stream::StorageType::File,
            discard: stream::DiscardPolicy::New,
            num_replicas: self.config.replicas,
            max_bytes: self.config.max_bytes,
            max_age: self.config.max_age,
            max_message_size: 65_536,
            duplicate_window: Duration::from_secs(60).min(self.config.max_age),
            ..Default::default()
        }
    }

    async fn provision(&self) -> Result<()> {
        for dead in [false, true] {
            let expected = self.stream_config(dead);
            let actual = self.context.get_or_create_stream(expected.clone()).await?;
            let config = &actual.cached_info().config;
            anyhow::ensure!(
                config.subjects == expected.subjects
                    && config.retention == expected.retention
                    && config.storage == expected.storage
                    && config.discard == expected.discard
                    && config.num_replicas == expected.num_replicas
                    && config.max_bytes == expected.max_bytes
                    && config.max_age == expected.max_age
                    && config.max_message_size == expected.max_message_size
                    && !config.no_ack
                    && !config.sealed
                    && config.duplicate_window == expected.duplicate_window,
                "JetStream stream configuration mismatch: {}",
                expected.name
            );
        }
        Ok(())
    }

    fn work_subject(&self, pool: WorkloadClass) -> String {
        format!("{}.jobs.{}", self.config.subject_prefix, pool.as_str())
    }

    pub async fn dead_letter_count(&self) -> Result<u64> {
        let stream = self
            .context
            .get_stream(format!("{}_DLQ", self.config.stream))
            .await?;
        Ok(stream.cached_info().state.messages)
    }

    pub async fn readiness(&self) -> Result<()> {
        // A management request verifies JetStream, not just the core TCP link.
        self.context.get_stream(&self.config.stream).await?;
        Ok(())
    }

    pub async fn consumer(&self, name: &str, pool: Option<WorkloadClass>) -> Result<Consumer> {
        anyhow::ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid durable consumer name"
        );
        let subject = pool
            .map(|pool| self.work_subject(pool))
            .unwrap_or_else(|| format!("{}.events", self.config.subject_prefix));
        let config = consumer::pull::Config {
            durable_name: Some(name.into()),
            filter_subject: subject,
            ack_policy: consumer::AckPolicy::Explicit,
            ack_wait: self.config.ack_wait,
            max_deliver: MAX_DELIVERIES as i64,
            max_ack_pending: 1024,
            max_batch: 256,
            max_waiting: 256,
            max_expires: Duration::from_secs(1),
            num_replicas: self.config.replicas,
            ..Default::default()
        };
        let stream = self.context.get_stream(&self.config.stream).await?;
        let inner: PullConsumer = stream.get_or_create_consumer(name, config.clone()).await?;
        let actual = &inner.cached_info().config;
        anyhow::ensure!(
            actual.durable_name == config.durable_name
                && actual.filter_subject == config.filter_subject
                && actual.ack_policy == config.ack_policy
                && actual.ack_wait == config.ack_wait
                && actual.max_deliver == config.max_deliver
                && actual.max_ack_pending == config.max_ack_pending
                && actual.max_batch == config.max_batch
                && actual.max_waiting == config.max_waiting
                && actual.max_expires == config.max_expires
                && actual.backoff.is_empty()
                && actual.num_replicas == config.num_replicas,
            "JetStream consumer configuration mismatch: {name}"
        );
        Ok(Consumer {
            inner,
            progress_interval: self.config.ack_wait / 3,
        })
    }

    /// Successful return means broker persistence ACK, never handler delivery.
    /// Timeout is ambiguous; retry with the same event+epoch message ID.
    pub async fn publish(&self, envelope: &EventEnvelope) -> Result<()> {
        let subject = if envelope.message_type == "deepref.work" {
            self.work_subject(envelope.work()?.workload_class)
        } else {
            format!("{}.events", self.config.subject_prefix)
        };
        let payload = serde_json::to_vec(envelope)?;
        anyhow::ensure!(
            payload.len() <= 65_536,
            "envelope exceeds transport size limit"
        );
        self.publish_bytes(
            subject,
            payload,
            format!("{}:{}", envelope.event_id, envelope.publication_epoch),
        )
        .await
    }

    async fn publish_bytes(
        &self,
        subject: String,
        payload: Vec<u8>,
        identity: String,
    ) -> Result<()> {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Nats-Msg-Id", identity);
        let confirmation = self
            .context
            .publish_with_headers(subject, headers, payload.into())
            .await?;
        confirmation.await?;
        Ok(())
    }

    /// Call only after the matching PostgreSQL dead-letter record commits.
    /// Malformed data uses stream sequence identity and never logs raw payload.
    pub async fn dead_letter(
        &self,
        consumer: &str,
        delivery: &Delivery,
        reason: &str,
    ) -> Result<()> {
        let envelope = delivery.envelope().ok();
        let sequence = delivery
            .message
            .info()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .stream_sequence;
        let record = serde_json::json!({"consumer": consumer, "event_id": envelope.as_ref().map(|e| e.event_id),
            "publication_epoch": envelope.as_ref().map(|e| e.publication_epoch), "stream_sequence": sequence,
            "reason": reason, "deliveries": delivery.deliveries()?, "recorded_at": chrono::Utc::now()});
        self.publish_bytes(
            format!("{}.dead.{consumer}", self.config.subject_prefix),
            serde_json::to_vec(&record)?,
            format!("dlq:{}:{consumer}:{sequence}", self.config.stream),
        )
        .await
    }
}

#[derive(Clone)]
pub struct Consumer {
    inner: PullConsumer,
    progress_interval: Duration,
}
impl Consumer {
    /// Immediate bounded fetch. Never leave a pending long pull behind a
    /// cancelled pool tick; no empty pool delays a busy sibling pool.
    pub async fn fetch(&self, limit: usize) -> Result<Vec<Delivery>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut batch = self
            .inner
            .fetch()
            .max_messages(limit.min(256))
            .expires(Duration::from_millis(20))
            .messages()
            .await?;
        let mut messages = Vec::new();
        while let Some(message) = batch.next().await {
            messages.push(Delivery::new(
                message.map_err(|e| anyhow::anyhow!(e.to_string()))?,
                self.progress_interval,
            ));
        }
        Ok(messages)
    }
    pub async fn visibility(&self) -> Result<(u64, usize, usize)> {
        let mut consumer = self.inner.clone();
        let info = consumer.info().await?;
        Ok((info.num_pending, info.num_ack_pending, info.num_redelivered))
    }
}

/// Owns progress heartbeats independently of DB lease renewal. Dropping it
/// stops heartbeats and allows redelivery; it never ACKs on drop.
pub struct Delivery {
    message: Arc<jetstream::Message>,
    progress: JoinHandle<()>,
}
impl Delivery {
    fn new(message: jetstream::Message, interval: Duration) -> Self {
        let message = Arc::new(message);
        let progress_message = Arc::clone(&message);
        let progress = tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                if let Err(error) = progress_message
                    .ack_with(jetstream::AckKind::Progress)
                    .await
                {
                    tracing::warn!(%error, "transport progress failed; DB fencing still applies");
                }
            }
        });
        Self { message, progress }
    }
    pub fn envelope(&self) -> Result<EventEnvelope> {
        let envelope: EventEnvelope = serde_json::from_slice(&self.message.payload)?;
        anyhow::ensure!(
            envelope.envelope_version == 1
                && envelope.message_version > 0
                && envelope.publication_epoch >= 0
                && !envelope.message_type.is_empty(),
            "unsupported envelope/version"
        );
        Ok(envelope)
    }
    pub fn identity(&self) -> Result<String> {
        let info = self
            .message
            .info()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(format!(
            "{}:{}:{}",
            info.stream, info.consumer, info.stream_sequence
        ))
    }
    pub fn deliveries(&self) -> Result<u64> {
        Ok(self
            .message
            .info()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .delivered as u64)
    }
    pub async fn ack(self) -> Result<()> {
        self.message
            .double_ack()
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
    pub async fn retry(self, delay: Duration) -> Result<()> {
        self.message
            .ack_with(jetstream::AckKind::Nak(Some(delay)))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
    pub async fn terminate(self) -> Result<()> {
        self.message
            .double_ack_with(jetstream::AckKind::Term)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        self.progress.abort();
    }
}
