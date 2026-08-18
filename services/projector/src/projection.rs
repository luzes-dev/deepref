use deepref_events::{DomainPayload, EventEnvelope};
use deepref_graph::{ApplyOutcome, GraphRepository, mutation_from_event};
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionOutcome {
    Applied,
    StaleOrDuplicate,
    Ignored,
}

pub async fn apply(
    pool: &PgPool,
    graph: &GraphRepository,
    event: &EventEnvelope<DomainPayload>,
) -> anyhow::Result<ProjectionOutcome> {
    let outcome = match &event.payload {
        DomainPayload::MetricsRecomputeRequested(payload) => {
            crate::metrics::recompute(pool, graph, payload.project_id, event.revision).await?;
            ProjectionOutcome::Applied
        }
        _ => match mutation_from_event(event) {
            Some(mutation) => match graph.apply_mutation(&mutation).await? {
                ApplyOutcome::Applied => ProjectionOutcome::Applied,
                ApplyOutcome::StaleOrDuplicate => ProjectionOutcome::StaleOrDuplicate,
            },
            None => return Ok(ProjectionOutcome::Ignored),
        },
    };
    crate::status::record_success(pool, event).await?;
    Ok(outcome)
}
