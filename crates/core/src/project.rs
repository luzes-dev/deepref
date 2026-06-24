use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub default_max_depth: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
