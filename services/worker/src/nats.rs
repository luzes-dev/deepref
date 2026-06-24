use async_nats::jetstream::{self, consumer::PullConsumer};
use deepref_events::{
    EventEnvelope, MetricsRecomputeRequested, STREAM, SUBJECT_METRICS_RECOMPUTE_REQUESTED,
    SUBJECT_WORK_FETCH_COMPLETED, SUBJECT_WORK_FETCH_FAILED, SUBJECT_WORK_FETCH_REQUESTED,
    WorkFetchCompleted, WorkFetchFailed, WorkFetchRequested,
};
use sqlx::PgPool;

use crate::outbox;

pub(crate) async fn enqueue_reference(
    pool: &PgPool,
    parent_event: &EventEnvelope<WorkFetchRequested>,
    parent: &WorkFetchRequested,
    doi: &str,
) -> anyhow::Result<()> {
    let payload = WorkFetchRequested {
        project_id: parent.project_id,
        ingestion_id: parent.ingestion_id,
        doi: doi.to_owned(),
        depth: parent.depth + 1,
        max_depth: parent.max_depth,
        parent_doi: Some(parent.doi.clone()),
    };
    let event = EventEnvelope::new(
        SUBJECT_WORK_FETCH_REQUESTED,
        "deepref.worker",
        format!("doi:{doi}"),
        parent.ingestion_id,
        Some(parent_event.id),
        payload,
    );
    outbox::enqueue(pool, event.id, SUBJECT_WORK_FETCH_REQUESTED, &event).await
}

pub(crate) async fn publish_completed(
    pool: &PgPool,
    payload: &WorkFetchRequested,
    doi: &str,
    references_discovered: usize,
) -> anyhow::Result<()> {
    let event = EventEnvelope::new(
        SUBJECT_WORK_FETCH_COMPLETED,
        "deepref.worker",
        format!("doi:{doi}"),
        payload.ingestion_id,
        None,
        WorkFetchCompleted {
            project_id: payload.project_id,
            ingestion_id: payload.ingestion_id,
            doi: doi.to_owned(),
            references_discovered,
        },
    );
    outbox::enqueue(pool, event.id, SUBJECT_WORK_FETCH_COMPLETED, &event).await
}

pub(crate) async fn publish_failed(
    pool: &PgPool,
    payload: &WorkFetchRequested,
    doi: &str,
    error: &str,
    retryable: bool,
) -> anyhow::Result<()> {
    let event = EventEnvelope::new(
        SUBJECT_WORK_FETCH_FAILED,
        "deepref.worker",
        format!("doi:{doi}"),
        payload.ingestion_id,
        None,
        WorkFetchFailed {
            project_id: payload.project_id,
            ingestion_id: payload.ingestion_id,
            doi: doi.to_owned(),
            error: error.to_owned(),
            retryable,
        },
    );
    outbox::enqueue(pool, event.id, SUBJECT_WORK_FETCH_FAILED, &event).await
}

pub(crate) async fn publish_metrics_recompute(
    pool: &PgPool,
    payload: &WorkFetchRequested,
) -> anyhow::Result<()> {
    let event = EventEnvelope::new(
        SUBJECT_METRICS_RECOMPUTE_REQUESTED,
        "deepref.worker",
        format!("project:{}", payload.project_id),
        payload.ingestion_id,
        None,
        MetricsRecomputeRequested {
            project_id: payload.project_id,
            ingestion_id: Some(payload.ingestion_id),
        },
    );
    outbox::enqueue(pool, event.id, SUBJECT_METRICS_RECOMPUTE_REQUESTED, &event).await
}

pub(crate) async fn ensure_stream_and_consumer(
    jetstream: &jetstream::Context,
) -> anyhow::Result<PullConsumer> {
    let stream = jetstream
        .get_or_create_stream(jetstream::stream::Config {
            name: STREAM.to_owned(),
            subjects: vec![
                "work.>".to_owned(),
                "metrics.>".to_owned(),
                "ingestion.>".to_owned(),
            ],
            ..Default::default()
        })
        .await?;

    Ok(stream
        .get_or_create_consumer(
            "deepref-worker",
            jetstream::consumer::pull::Config {
                durable_name: Some("deepref-worker".to_owned()),
                filter_subject: SUBJECT_WORK_FETCH_REQUESTED.to_owned(),
                ..Default::default()
            },
        )
        .await?)
}
