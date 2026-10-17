use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use utoipa::ToSchema;

use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SettingsDto {
    default_max_depth: i32,
    max_concurrency: i32,
    rate_limit_per_second: i32,
    retry_attempts: i32,
    metadata_provider: String,
    citation_provider: String,
}

#[utoipa::path(
    get,
    path = "/settings",
    operation_id = "getSettings",
    tag = "settings",
    responses(
        (status = 200, description = "Application settings", body = SettingsDto),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_settings(
    State(state): State<AppState>,
) -> Result<Json<SettingsDto>, ApiError> {
    ensure_settings(&state.pool).await?;
    let row = sqlx::query(
        r#"
        SELECT default_max_depth, max_concurrency, rate_limit_per_second,
               retry_attempts, metadata_provider, citation_provider
        FROM settings WHERE id = 1
        "#,
    )
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(SettingsDto {
        default_max_depth: row.get("default_max_depth"),
        max_concurrency: row.get("max_concurrency"),
        rate_limit_per_second: row.get("rate_limit_per_second"),
        retry_attempts: row.get("retry_attempts"),
        metadata_provider: row.get("metadata_provider"),
        citation_provider: row.get("citation_provider"),
    }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct UpdateSettings {
    default_max_depth: Option<i32>,
    max_concurrency: Option<i32>,
    rate_limit_per_second: Option<i32>,
    retry_attempts: Option<i32>,
}

impl UpdateSettings {
    fn validate(&self) -> Result<(), ApiError> {
        if let Some(value) = self.default_max_depth {
            deepref_domain::acquisition::MaxDepth::try_from(value)
                .map_err(|message| ApiError::BadRequest(message.to_owned()))?;
        }
        reject_less_than_one("max_concurrency", self.max_concurrency)?;
        reject_less_than_one("rate_limit_per_second", self.rate_limit_per_second)?;
        reject_less_than_one("retry_attempts", self.retry_attempts)?;
        Ok(())
    }
}

#[utoipa::path(
    patch,
    path = "/settings",
    operation_id = "updateSettings",
    tag = "settings",
    request_body = UpdateSettings,
    responses(
        (status = 200, description = "Updated application settings", body = SettingsDto),
        (status = 400, description = "Invalid settings", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn update_settings(
    State(state): State<AppState>,
    Json(input): Json<UpdateSettings>,
) -> Result<Json<SettingsDto>, ApiError> {
    input.validate()?;
    ensure_settings(&state.pool).await?;
    sqlx::query(
        r#"
        UPDATE settings SET
          default_max_depth = COALESCE($1, default_max_depth),
          max_concurrency = COALESCE($2, max_concurrency),
          rate_limit_per_second = COALESCE($3, rate_limit_per_second),
          retry_attempts = COALESCE($4, retry_attempts),
          updated_at = now()
        WHERE id = 1
        "#,
    )
    .bind(input.default_max_depth)
    .bind(input.max_concurrency)
    .bind(input.rate_limit_per_second)
    .bind(input.retry_attempts)
    .execute(&state.pool)
    .await?;
    get_settings(State(state)).await
}

pub(crate) async fn ensure_settings(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO settings (id)
        VALUES (1)
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn reject_less_than_one(name: &str, value: Option<i32>) -> Result<(), ApiError> {
    if value.is_some_and(|value| value < 1) {
        return Err(ApiError::BadRequest(format!("{name} must be >= 1")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_settings() {
        assert!(
            UpdateSettings {
                default_max_depth: Some(4),
                max_concurrency: None,
                rate_limit_per_second: None,
                retry_attempts: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            UpdateSettings {
                default_max_depth: None,
                max_concurrency: Some(0),
                rate_limit_per_second: None,
                retry_attempts: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            UpdateSettings {
                default_max_depth: Some(3),
                max_concurrency: Some(1),
                rate_limit_per_second: Some(1),
                retry_attempts: Some(1),
            }
            .validate()
            .is_ok()
        );
    }
}
