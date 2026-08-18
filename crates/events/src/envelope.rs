use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::EntityType;

pub const SCHEMA_VERSION_V1: u16 = 1;
const EVENT_NAMESPACE: Uuid = Uuid::from_u128(0xd3ed_5c52_3587_5f9b_a3f2_b093_5bda_955c);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventEnvelope<T> {
    #[serde(default = "schema_version_v1")]
    pub schema_version: u16,
    #[serde(alias = "id")]
    pub event_id: Uuid,
    #[serde(alias = "type")]
    pub event_type: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(alias = "source")]
    pub producer: String,
    pub correlation_id: Uuid,
    #[serde(default)]
    pub causation_id: Option<Uuid>,
    #[serde(default)]
    pub entity_type: EntityType,
    #[serde(alias = "subject")]
    pub entity_key: String,
    #[serde(default)]
    pub revision: i64,
    pub payload: T,
}

const fn schema_version_v1() -> u16 {
    SCHEMA_VERSION_V1
}

pub fn deterministic_event_id(
    schema_version: u16,
    event_type: &str,
    entity_type: EntityType,
    entity_key: &str,
    revision: i64,
) -> Uuid {
    let canonical = format!(
        "{schema_version}|{event_type}|{}|{entity_key}|{revision}",
        entity_type.as_str()
    );
    Uuid::new_v5(&EVENT_NAMESPACE, canonical.as_bytes())
}

impl<T> EventEnvelope<T> {
    #[allow(clippy::too_many_arguments)]
    pub fn v1(
        event_type: impl Into<String>,
        producer: impl Into<String>,
        entity_type: EntityType,
        entity_key: impl Into<String>,
        revision: i64,
        correlation_id: Uuid,
        causation_id: Option<Uuid>,
        payload: T,
    ) -> Self {
        let event_type = event_type.into();
        let entity_key = entity_key.into();
        Self {
            schema_version: SCHEMA_VERSION_V1,
            event_id: deterministic_event_id(
                SCHEMA_VERSION_V1,
                &event_type,
                entity_type,
                &entity_key,
                revision,
            ),
            event_type,
            occurred_at: Utc::now(),
            producer: producer.into(),
            correlation_id,
            causation_id,
            entity_type,
            entity_key,
            revision,
            payload,
        }
    }

    /// Transitional constructor retained for current publishers. It emits V1 and
    /// uses revision zero until the caller has persisted an entity revision.
    pub fn new(
        event_type: impl Into<String>,
        producer: impl Into<String>,
        entity_key: impl Into<String>,
        correlation_id: Uuid,
        causation_id: Option<Uuid>,
        payload: T,
    ) -> Self {
        Self::v1(
            event_type,
            producer,
            EntityType::Work,
            entity_key,
            0,
            correlation_id,
            causation_id,
            payload,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkFetchRequested;

    fn event() -> EventEnvelope<WorkFetchRequested> {
        EventEnvelope::v1(
            "work.fetch.requested",
            "test",
            EntityType::Work,
            "doi:10.1/x",
            42,
            Uuid::nil(),
            None,
            WorkFetchRequested {
                project_id: Uuid::nil(),
                ingestion_id: Uuid::nil(),
                doi: "10.1/x".to_owned(),
                depth: 0,
                max_depth: 2,
                parent_doi: None,
            },
        )
    }

    #[test]
    fn deterministic_id_uses_canonical_identity_tuple() {
        assert_eq!(event().event_id, event().event_id);
        assert_ne!(
            event().event_id,
            deterministic_event_id(
                1,
                "work.fetch.requested",
                EntityType::Work,
                "doi:10.1/x",
                43
            )
        );
    }

    #[test]
    fn serializes_v1_field_names() {
        let value = serde_json::to_value(event()).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["event_type"], "work.fetch.requested");
        assert!(value.get("id").is_none());
    }
}
