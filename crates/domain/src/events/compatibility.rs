use serde::de::DeserializeOwned;

use super::{EventEnvelope, SCHEMA_VERSION_V1};

/// Deserializes an event envelope, rejecting unsupported schema versions.
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
    use uuid::Uuid;

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
