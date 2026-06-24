use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope<T> {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub event_type: String,
    pub source: String,
    pub subject: String,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    pub occurred_at: DateTime<Utc>,
    pub payload: T,
}

impl<T> EventEnvelope<T> {
    pub fn new(
        event_type: impl Into<String>,
        source: impl Into<String>,
        subject: impl Into<String>,
        correlation_id: Uuid,
        causation_id: Option<Uuid>,
        payload: T,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type: event_type.into(),
            source: source.into(),
            subject: subject.into(),
            correlation_id,
            causation_id,
            occurred_at: Utc::now(),
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkFetchRequested;

    #[test]
    fn serializes_event_type_as_type() {
        let event = EventEnvelope::new(
            "work.fetch.requested",
            "test",
            "doi:10.1/x",
            Uuid::new_v4(),
            None,
            WorkFetchRequested {
                project_id: Uuid::new_v4(),
                ingestion_id: Uuid::new_v4(),
                doi: "10.1/x".to_owned(),
                depth: 0,
                max_depth: 2,
                parent_doi: None,
            },
        );
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "work.fetch.requested");
    }
}
