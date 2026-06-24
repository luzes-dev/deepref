use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct HealthResponse {
    status: &'static str,
}

#[utoipa::path(
    get,
    path = "/health",
    operation_id = "getHealth",
    tag = "health",
    responses((status = 200, description = "API is healthy", body = HealthResponse))
)]
pub(crate) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}
