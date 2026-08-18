use serde::de::DeserializeOwned;

use crate::{EventEnvelope, SCHEMA_VERSION_V1};

/// Accepts the one-release legacy fetch envelope through serde aliases and
/// defaults, while rejecting unknown future schema versions.
pub fn deserialize_compatible<T: DeserializeOwned>(
    bytes: &[u8],
) -> Result<EventEnvelope<T>, serde_json::Error> {
    let envelope: EventEnvelope<T> = serde_json::from_slice(bytes)?;
    if envelope.schema_version != SCHEMA_VERSION_V1 {
        return Err(<serde_json::Error as serde::de::Error>::custom(format!(
            "unsupported schema_version {}",
            envelope.schema_version
        )));
    }
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntityType, WorkFetchRequested};
    use uuid::Uuid;

    #[test]
    fn accepts_legacy_fetch_field_names() {
        let id = Uuid::new_v4();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "id": id,
            "type": "work.fetch.requested",
            "occurred_at": "2026-01-01T00:00:00Z",
            "source": "legacy-api",
            "correlation_id": Uuid::nil(),
            "subject": "10.1/x",
            "payload": {
                "project_id": Uuid::nil(), "ingestion_id": Uuid::nil(),
                "doi": "10.1/x", "depth": 0, "max_depth": 2, "parent_doi": null
            }
        }))
        .unwrap();
        let event: EventEnvelope<WorkFetchRequested> = deserialize_compatible(&bytes).unwrap();
        assert_eq!(event.event_id, id);
        assert_eq!(event.entity_type, EntityType::Work);
        assert_eq!(event.revision, 0);
    }

    #[test]
    fn rejects_future_versions() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schema_version": 99, "event_id": Uuid::nil(), "event_type": "x",
            "occurred_at": "2026-01-01T00:00:00Z", "producer": "x",
            "correlation_id": Uuid::nil(), "entity_type": "work", "entity_key": "x",
            "revision": 1, "payload": {}
        }))
        .unwrap();
        assert!(deserialize_compatible::<serde_json::Value>(&bytes).is_err());
    }
}
