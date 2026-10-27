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

/// Low-cardinality observations for the parent telemetry/health bridge.
/// Implementations must be synchronous, nonblocking and must not attach event
/// IDs, payloads or error text as metric labels. No exporter/alerts live here.
#[derive(Debug, Clone)]
pub enum TransportObservation {
    Readiness {
        ready: bool,
    },
    Visibility {
        pool: WorkloadClass,
        pending: u64,
        in_flight: usize,
        redeliveries: usize,
    },
    Claim {
        pool: WorkloadClass,
        outcome: ClaimOutcome,
        elapsed: Duration,
    },
    ConsumeLatency {
        elapsed: Duration,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    Claimed,
    Noop,
    Retry,
    Failed,
}
pub trait TransportObserver: Send + Sync {
    fn observe(&self, observation: TransportObservation);
}
struct NoopObserver;
impl TransportObserver for NoopObserver {
    fn observe(&self, _: TransportObservation) {}
}

/// Metadata only; this is not an execution capability or a business retry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TerminalAdvisory {
    #[serde(rename = "type")]
    pub kind: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub stream: String,
    pub consumer: String,
    pub stream_seq: u64,
    pub deliveries: u64,
}

impl TerminalAdvisory {
    fn decode(bytes: &[u8], expected_stream: &str) -> Result<Self> {
        anyhow::ensure!(
            bytes.len() <= 16_384,
            "terminal advisory metadata exceeds budget"
        );
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("invalid terminal advisory metadata"))?;
        anyhow::ensure!(
            value.kind == "io.nats.jetstream.advisory.v1.max_deliver"
                && value.stream == expected_stream
                && value.stream_seq > 0
                && value.deliveries == MAX_DELIVERIES
                && !value.consumer.is_empty()
                && value.consumer.len() <= 128
                && value
                    .consumer
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "terminal advisory does not match the configured finite delivery contract"
        );
        Ok(value)
    }
}

#[derive(Debug, serde::Serialize)]
pub struct RetainedTerminalAdvisory {
    pub advisory_sequence: u64,
    pub advisory: TerminalAdvisory,
}

#[derive(Debug, serde::Serialize)]
pub struct TerminalAdvisoryPage {
    pub retained_first_sequence: u64,
    pub observed_last_sequence: u64,
    pub retained_count: u64,
    pub next_after: u64,
    pub complete_to_observed_end: bool,
    pub entries: Vec<RetainedTerminalAdvisory>,
}

#[derive(Clone, Copy)]
enum StreamPurpose {
    Work,
    DeadLetter,
    TerminalEvidence,
}

#[derive(Clone)]
pub struct JetStream {
    context: jetstream::Context,
    config: NatsConfig,
    observer: Arc<dyn TransportObserver>,
}

impl JetStream {
    /// Provision absent resources, validate existing resources exactly. A
    /// rolling deployment must never silently mutate retention or ACK policy.
    pub async fn connect(config: NatsConfig) -> Result<Self> {
        let adapter = Self::connect_client(config).await?;
        adapter.provision().await?;
        Ok(adapter)
    }

    /// Operator access never creates missing streams or consumers.
    pub async fn connect_existing(config: NatsConfig) -> Result<Self> {
        let adapter = Self::connect_client(config).await?;
        adapter.readiness().await?;
        Ok(adapter)
    }

    async fn connect_client(config: NatsConfig) -> Result<Self> {
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
        let adapter = Self {
            context,
            config,
            observer: Arc::new(NoopObserver),
        };
        Ok(adapter)
    }

    pub fn with_observer(mut self, observer: Arc<dyn TransportObserver>) -> Self {
        self.observer = observer;
        self
    }
    pub fn observe(&self, observation: TransportObservation) {
        self.observer.observe(observation);
    }

    fn stream_config(&self, kind: StreamPurpose) -> stream::Config {
        stream::Config {
            name: match kind {
                StreamPurpose::DeadLetter => format!("{}_DLQ", self.config.stream),
                StreamPurpose::TerminalEvidence => format!("{}_TERMINAL", self.config.stream),
                StreamPurpose::Work => self.config.stream.clone(),
            },
            subjects: match kind {
                StreamPurpose::DeadLetter => vec![format!("{}.dead.>", self.config.subject_prefix)],
                // MaxDeliver advisories contain stream/consumer/sequence, not
                // the original payload. Retain bounded operator evidence even
                // if the worker dies on its fifth delivery.
                StreamPurpose::TerminalEvidence => vec![format!(
                    "$JS.EVENT.ADVISORY.CONSUMER.MAX_DELIVERIES.{}.*",
                    self.config.stream
                )],
                StreamPurpose::Work => vec![
                    format!("{}.jobs.*", self.config.subject_prefix),
                    format!("{}.events", self.config.subject_prefix),
                ],
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
        for kind in [
            StreamPurpose::Work,
            StreamPurpose::DeadLetter,
            StreamPurpose::TerminalEvidence,
        ] {
            let expected = self.stream_config(kind);
            let actual = self.context.get_or_create_stream(expected.clone()).await?;
            let config = &actual.cached_info().config;
            self.validate_stream(kind, config)?;
        }
        Ok(())
    }

    fn validate_stream(&self, kind: StreamPurpose, config: &stream::Config) -> Result<()> {
        let expected = self.stream_config(kind);
        anyhow::ensure!(
            config.name == expected.name
                && config.subjects == expected.subjects
                && config.retention == expected.retention
                && config.storage == expected.storage
                && config.discard == expected.discard
                && config.num_replicas == expected.num_replicas
                && config.max_bytes == expected.max_bytes
                && config.max_age == expected.max_age
                && config.max_message_size == expected.max_message_size
                && !config.no_ack
                && !config.sealed
                && !config.discard_new_per_subject
                && config.max_messages <= 0
                && config.max_messages_per_subject <= 0
                && config.max_consumers <= 0
                && config.template_owner.is_empty()
                && config.subject_transform.is_none()
                && config.republish.is_none()
                && config.mirror.is_none()
                && config.sources.as_ref().is_none_or(Vec::is_empty)
                && !config.mirror_direct
                && !config.allow_direct
                && !config.allow_rollup
                && !config.deny_delete
                && !config.deny_purge
                && config.consumer_limits.is_none()
                && config.pause_until.is_none()
                && !config.allow_message_ttl
                && config.subject_delete_marker_ttl.is_none()
                && !config.allow_message_schedules
                && !config.allow_message_counter
                && !config.allow_atomic_publish
                && !config.allow_batch_publish
                && config.duplicate_window == expected.duplicate_window,
            "JetStream stream configuration mismatch: {}",
            expected.name
        );
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

    /// Metadata-only terminal evidence survives exhaustion independently of
    /// DB/DLQ availability. It is bounded by the configured byte/age limits.
    pub async fn terminal_evidence_count(&self) -> Result<u64> {
        let stream = self
            .context
            .get_stream(format!("{}_TERMINAL", self.config.stream))
            .await?;
        Ok(stream.cached_info().state.messages)
    }

    /// Bounded, read-only page of retained metadata advisories. A gap or expired
    /// cursor is incomplete evidence, never a claim that no failures occurred.
    pub async fn terminal_advisories(
        &self,
        after: u64,
        limit: u16,
    ) -> Result<TerminalAdvisoryPage> {
        anyhow::ensure!(
            (1..=100).contains(&limit),
            "terminal advisory limit must be 1..100"
        );
        let stream = self
            .context
            .get_stream(format!("{}_TERMINAL", self.config.stream))
            .await?;
        let state = &stream.cached_info().state;
        let first = state.first_sequence;
        let last = state.last_sequence;
        anyhow::ensure!(
            after <= last,
            "terminal advisory cursor is ahead of the observed stream"
        );
        let start = if after == 0 {
            first
        } else {
            let next = after
                .checked_add(1)
                .context("terminal advisory cursor exhausted")?;
            anyhow::ensure!(
                first == 0 || next >= first,
                "terminal advisory cursor expired; inspect retained first sequence before resuming"
            );
            next
        };
        let mut entries = Vec::new();
        let mut cursor = after;
        if state.messages > 0 && start <= last {
            for sequence in start..=last {
                let message = stream
                    .get_raw_message(sequence)
                    .await
                    .context("terminal advisory page incomplete; retained sequence unavailable")?;
                let advisory = TerminalAdvisory::decode(&message.payload, &self.config.stream)?;
                entries.push(RetainedTerminalAdvisory {
                    advisory_sequence: sequence,
                    advisory,
                });
                cursor = sequence;
                if entries.len() >= usize::from(limit) {
                    break;
                }
            }
        }
        Ok(TerminalAdvisoryPage {
            retained_first_sequence: first,
            observed_last_sequence: last,
            retained_count: state.messages,
            next_after: cursor,
            complete_to_observed_end: state.messages == 0 || cursor >= last,
            entries,
        })
    }

    pub async fn terminal_advisory(&self, sequence: u64) -> Result<TerminalAdvisory> {
        anyhow::ensure!(sequence > 0, "terminal advisory sequence must be positive");
        let stream = self
            .context
            .get_stream(format!("{}_TERMINAL", self.config.stream))
            .await?;
        let message = stream
            .get_raw_message(sequence)
            .await
            .context("terminal advisory unavailable; no recovery evidence can be inferred")?;
        TerminalAdvisory::decode(&message.payload, &self.config.stream)
    }

    pub fn sequence_identity(&self, consumer: &str, sequence: u64) -> String {
        format!("{}:{consumer}:{sequence}", self.config.stream)
    }

    /// Distinguish unavailable/expired retained evidence from a malformed body.
    pub async fn retained_sequence(&self, sequence: u64) -> Result<Option<EventEnvelope>> {
        let stream = self.context.get_stream(&self.config.stream).await?;
        let message = stream.get_raw_message(sequence).await?;
        Ok(EventEnvelope::decode(&message.payload).ok())
    }

    /// Maintenance recovery after final-delivery failure. The caller must
    /// first commit matching DB evidence, as in normal rejection. No reset of
    /// consumer delivery/business budgets and no deletion of a shared message.
    pub async fn dead_letter_sequence(
        &self,
        consumer: &str,
        sequence: u64,
        deliveries: u64,
        reason: &str,
    ) -> Result<()> {
        let stream = self.context.get_stream(&self.config.stream).await?;
        let message = stream.get_raw_message(sequence).await?;
        let envelope = EventEnvelope::decode(&message.payload).ok();
        let record = serde_json::json!({"consumer":consumer,"event_id":envelope.as_ref().map(|e|e.event_id),
            "publication_epoch":envelope.as_ref().map(|e|e.publication_epoch),"stream_sequence":sequence,
            "reason":reason,"deliveries":deliveries,"recorded_at":chrono::Utc::now()});
        self.publish_bytes(
            format!("{}.dead.{consumer}", self.config.subject_prefix),
            serde_json::to_vec(&record)?,
            format!("dlq:{}:{consumer}:{sequence}", self.config.stream),
        )
        .await
    }

    pub async fn readiness(&self) -> Result<()> {
        // Management requests verify all required JetStream resources, not
        // merely the core TCP link. The supervisor bounds the combined sample.
        let result = async {
            for kind in [
                StreamPurpose::Work,
                StreamPurpose::DeadLetter,
                StreamPurpose::TerminalEvidence,
            ] {
                let stream = self
                    .context
                    .get_stream(self.stream_config(kind).name)
                    .await?;
                self.validate_stream(kind, &stream.cached_info().config)?;
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        self.observe(TransportObservation::Readiness {
            ready: result.is_ok(),
        });
        result
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
                && actual.num_replicas == config.num_replicas
                && !actual.headers_only
                && actual.deliver_policy == consumer::DeliverPolicy::All
                && actual.replay_policy == consumer::ReplayPolicy::Instant
                && actual.inactive_threshold.is_zero()
                && !actual.memory_storage
                && actual.deliver_subject.is_none()
                && actual.deliver_group.is_none()
                && actual.filter_subjects.is_empty()
                && actual.rate_limit == 0
                && !actual.flow_control
                && actual.idle_heartbeat.is_zero()
                && actual.max_bytes == 0
                && actual.pause_until.is_none()
                && actual.priority_policy == consumer::PriorityPolicy::None
                && actual.priority_groups.is_empty(),
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
        Ok(EventEnvelope::decode(&self.message.payload)?)
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

#[cfg(test)]
mod operator_tests {
    use super::*;
    #[test]
    fn advisory_validation_refuses_foreign_contracts_and_redacts_bad_payload() -> Result<()> {
        let valid = serde_json::json!({"type":"io.nats.jetstream.advisory.v1.max_deliver",
            "timestamp":chrono::Utc::now(),"stream":"WORK","consumer":"work-documents-v1","stream_seq":7,"deliveries":5});
        let parsed = TerminalAdvisory::decode(&serde_json::to_vec(&valid)?, "WORK")?;
        anyhow::ensure!(parsed.stream_seq == 7);
        for (field, bad) in [
            ("stream", serde_json::json!("FOREIGN")),
            ("deliveries", serde_json::json!(6)),
            ("stream_seq", serde_json::json!(0)),
            ("consumer", serde_json::json!("secret.invalid")),
        ] {
            let mut value = valid.clone();
            value[field] = bad;
            anyhow::ensure!(
                TerminalAdvisory::decode(&serde_json::to_vec(&value)?, "WORK").is_err()
            );
        }
        let error = TerminalAdvisory::decode(br#"{"private-token":"never-print-this"}"#, "WORK")
            .err()
            .context("expected malformed metadata refusal")?;
        anyhow::ensure!(!format!("{error:#}").contains("never-print-this"));
        Ok(())
    }
}
