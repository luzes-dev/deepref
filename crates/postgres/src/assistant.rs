use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AssistantError {
    #[error("assistant database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("assistant conversation not found")]
    ConversationNotFound,
    #[error("project not found")]
    ProjectNotFound,
    #[error("assistant input is invalid: {0}")]
    InvalidInput(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantConversationRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessageRecord {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Value>,
    pub tool_results: Option<Value>,
    pub metadata: Option<Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendAssistantMessage {
    pub id: Option<Uuid>,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Value>,
    pub tool_results: Option<Value>,
    pub metadata: Option<Value>,
}

fn conversation_from_row(row: &PgRow) -> AssistantConversationRecord {
    AssistantConversationRecord {
        id: row.get("id"),
        project_id: row.get("project_id"),
        title: row.get("title"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn message_from_row(row: &PgRow) -> AssistantMessageRecord {
    AssistantMessageRecord {
        id: row.get("id"),
        conversation_id: row.get("conversation_id"),
        role: row.get("role"),
        content: row.get("content"),
        tool_calls: row.get("tool_calls"),
        tool_results: row.get("tool_results"),
        metadata: row.get("metadata"),
        created_at: row.get("created_at"),
    }
}

pub async fn create_assistant_conversation(
    pool: &PgPool,
    project_id: Uuid,
    title: &str,
) -> Result<AssistantConversationRecord, AssistantError> {
    if project_id.is_nil() {
        return Err(AssistantError::InvalidInput(
            "project_id must not be nil".to_owned(),
        ));
    }
    let trimmed = title.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 500 {
        return Err(AssistantError::InvalidInput(
            "title must be between 1 and 500 characters".to_owned(),
        ));
    }
    let row = sqlx::query(
        "INSERT INTO assistant_conversations (id, project_id, title, created_at, updated_at)
         VALUES ($1, $2, $3, now(), now())
         RETURNING id, project_id, title, created_at, updated_at",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(trimmed)
    .fetch_one(pool)
    .await
    .map_err(|err| {
        if let sqlx::Error::Database(ref db_err) = err
            && db_err.code().as_deref() == Some("23503")
        {
            return AssistantError::ProjectNotFound;
        }
        AssistantError::Database(err)
    })?;

    Ok(conversation_from_row(&row))
}

pub async fn list_assistant_conversations(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<Vec<AssistantConversationRecord>, AssistantError> {
    if project_id.is_nil() {
        return Err(AssistantError::InvalidInput(
            "project_id must not be nil".to_owned(),
        ));
    }
    let rows = sqlx::query(
        "SELECT id, project_id, title, created_at, updated_at
         FROM assistant_conversations
         WHERE project_id = $1
         ORDER BY updated_at DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(conversation_from_row).collect())
}

pub async fn get_assistant_conversation(
    pool: &PgPool,
    project_id: Uuid,
    conversation_id: Uuid,
) -> Result<AssistantConversationRecord, AssistantError> {
    if project_id.is_nil() || conversation_id.is_nil() {
        return Err(AssistantError::InvalidInput(
            "identifiers must not be nil".to_owned(),
        ));
    }
    let row = sqlx::query(
        "SELECT id, project_id, title, created_at, updated_at
         FROM assistant_conversations
         WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(conversation_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AssistantError::ConversationNotFound)?;

    Ok(conversation_from_row(&row))
}

pub async fn delete_assistant_conversation(
    pool: &PgPool,
    project_id: Uuid,
    conversation_id: Uuid,
) -> Result<bool, AssistantError> {
    if project_id.is_nil() || conversation_id.is_nil() {
        return Err(AssistantError::InvalidInput(
            "identifiers must not be nil".to_owned(),
        ));
    }
    let result = sqlx::query(
        "DELETE FROM assistant_conversations
         WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(conversation_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn append_assistant_message(
    pool: &PgPool,
    message: &AppendAssistantMessage,
) -> Result<AssistantMessageRecord, AssistantError> {
    if message.conversation_id.is_nil() {
        return Err(AssistantError::InvalidInput(
            "conversation_id must not be nil".to_owned(),
        ));
    }
    if !matches!(
        message.role.as_str(),
        "user" | "assistant" | "system" | "tool"
    ) {
        return Err(AssistantError::InvalidInput(
            "role must be user, assistant, system, or tool".to_owned(),
        ));
    }

    let mut tx = pool.begin().await?;

    let message_id = message.id.unwrap_or_else(Uuid::new_v4);

    let row = sqlx::query(
        "INSERT INTO assistant_messages (id, conversation_id, role, content, tool_calls, tool_results, metadata, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())
         RETURNING id, conversation_id, role, content, tool_calls, tool_results, metadata, created_at",
    )
    .bind(message_id)
    .bind(message.conversation_id)
    .bind(&message.role)
    .bind(&message.content)
    .bind(&message.tool_calls)
    .bind(&message.tool_results)
    .bind(&message.metadata)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if let sqlx::Error::Database(ref db_err) = err
            && db_err.code().as_deref() == Some("23503")
        {
            return AssistantError::ConversationNotFound;
        }
        AssistantError::Database(err)
    })?;

    sqlx::query(
        "UPDATE assistant_conversations
         SET updated_at = now()
         WHERE id = $1",
    )
    .bind(message.conversation_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(message_from_row(&row))
}

pub async fn list_assistant_messages(
    pool: &PgPool,
    conversation_id: Uuid,
) -> Result<Vec<AssistantMessageRecord>, AssistantError> {
    if conversation_id.is_nil() {
        return Err(AssistantError::InvalidInput(
            "conversation_id must not be nil".to_owned(),
        ));
    }
    let rows = sqlx::query(
        "SELECT id, conversation_id, role, content, tool_calls, tool_results, metadata, created_at
         FROM assistant_messages
         WHERE conversation_id = $1
         ORDER BY created_at ASC",
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(message_from_row).collect())
}
