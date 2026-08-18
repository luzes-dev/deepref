use deepref_events::{DomainPayload, EntityType, EventEnvelope};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct GraphMutation {
    pub event_id: Uuid,
    pub entity_type: EntityType,
    pub entity_key: String,
    pub revision: i64,
    pub payload: DomainPayload,
}

pub const fn cursor_type(entity_type: EntityType) -> &'static str {
    entity_type.as_str()
}

pub fn mutation_from_event(event: &EventEnvelope<DomainPayload>) -> Option<GraphMutation> {
    let mutates_graph = matches!(
        event.payload,
        DomainPayload::WorkUpserted(_)
            | DomainPayload::WorkTombstoned(_)
            | DomainPayload::ProjectMembershipUpserted(_)
            | DomainPayload::ProjectMembershipTombstoned(_)
            | DomainPayload::CitationUpserted(_)
            | DomainPayload::CitationTombstoned(_)
            | DomainPayload::UnresolvedReferenceUpserted(_)
            | DomainPayload::UnresolvedReferenceTombstoned(_)
            | DomainPayload::ProjectTombstoned(_)
    );
    mutates_graph.then(|| GraphMutation {
        event_id: event.event_id,
        entity_type: event.entity_type,
        entity_key: event.entity_key.clone(),
        revision: event.revision,
        payload: event.payload.clone(),
    })
}

#[cfg(test)]
mod tests {
    use deepref_events::{EntityType, EventEnvelope, MetricsRecomputeRequested};

    use super::*;

    #[test]
    fn metric_requests_do_not_mutate_graph() {
        let event = EventEnvelope::v1(
            "domain.metrics.recompute.requested.v1",
            "test",
            EntityType::Metric,
            "project",
            1,
            Uuid::nil(),
            None,
            DomainPayload::MetricsRecomputeRequested(MetricsRecomputeRequested {
                project_id: Uuid::nil(),
                ingestion_id: None,
            }),
        );
        assert!(mutation_from_event(&event).is_none());
    }
}
