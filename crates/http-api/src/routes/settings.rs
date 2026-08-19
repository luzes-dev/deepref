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
    crossref_mailto: String,
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
        SELECT crossref_mailto, default_max_depth, max_concurrency, rate_limit_per_second,
               retry_attempts, metadata_provider, citation_provider
        FROM settings WHERE id = 1
        "#,
    )
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(SettingsDto {
        crossref_mailto: row.get("crossref_mailto"),
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
    crossref_mailto: Option<String>,
    default_max_depth: Option<i32>,
    max_concurrency: Option<i32>,
    rate_limit_per_second: Option<i32>,
    retry_attempts: Option<i32>,
}

impl UpdateSettings {
    fn validate(&self) -> Result<(), ApiError> {
        if self
            .crossref_mailto
            .as_deref()
            .is_some_and(|mailto| mailto.trim().is_empty())
        {
            return Err(ApiError::BadRequest(
                "crossref_mailto must not be blank".to_owned(),
            ));
        }
        reject_negative("default_max_depth", self.default_max_depth)?;
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
          crossref_mailto = COALESCE($1, crossref_mailto),
          default_max_depth = COALESCE($2, default_max_depth),
          max_concurrency = COALESCE($3, max_concurrency),
          rate_limit_per_second = COALESCE($4, rate_limit_per_second),
          retry_attempts = COALESCE($5, retry_attempts),
          updated_at = now()
        WHERE id = 1
        "#,
    )
    .bind(input.crossref_mailto.map(|mailto| mailto.trim().to_owned()))
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
        INSERT INTO settings (id, crossref_mailto)
        VALUES (1, '')
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn reject_negative(name: &str, value: Option<i32>) -> Result<(), ApiError> {
    if value.is_some_and(|value| value < 0) {
        return Err(ApiError::BadRequest(format!("{name} must be >= 0")));
    }
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
                crossref_mailto: Some(" ".to_owned()),
                default_max_depth: None,
                max_concurrency: None,
                rate_limit_per_second: None,
                retry_attempts: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            UpdateSettings {
                crossref_mailto: None,
                default_max_depth: None,
                max_concurrency: Some(0),
                rate_limit_per_second: None,
                retry_attempts: None,
            }
            .validate()
            .is_err()
        );
    }
}
