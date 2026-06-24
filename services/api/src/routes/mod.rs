mod articles;
mod health;
mod ingestions;
mod projects;
mod settings;

use std::sync::Arc;

use axum::{
    Json, Router,
    http::{Method, header},
    routing::get,
};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use utoipa::openapi::{Info, OpenApi};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{config::cors_origins, state::AppState};

fn openapi_router() -> OpenApiRouter<AppState> {
    let mut openapi = OpenApi::default();
    openapi.info = Info::new("DeepRef API", env!("CARGO_PKG_VERSION"));

    OpenApiRouter::with_openapi(openapi)
        .routes(routes!(health::health))
        .routes(routes!(settings::get_settings, settings::update_settings))
        .routes(routes!(projects::list_projects, projects::create_project))
        .routes(routes!(
            projects::get_project,
            projects::update_project,
            projects::delete_project
        ))
        .routes(routes!(articles::list_articles))
        .routes(routes!(articles::get_article))
        .routes(routes!(articles::project_graph))
        .routes(routes!(articles::recommendations))
        .routes(routes!(articles::recompute_metrics))
        .routes(routes!(
            ingestions::list_ingestions,
            ingestions::create_ingestion
        ))
        .routes(routes!(ingestions::get_ingestion))
        .routes(routes!(ingestions::list_ingestion_items))
        .routes(routes!(ingestions::cancel_ingestion))
}

pub(crate) fn openapi_document() -> OpenApi {
    openapi_router().into_openapi()
}

pub(crate) fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(cors_origins())
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE]);

    let (router, openapi) = openapi_router().split_for_parts();
    let openapi = Arc::new(openapi);

    router
        .route("/openapi.json", {
            let openapi = Arc::clone(&openapi);
            get(move || {
                let openapi = Arc::clone(&openapi);
                async move { Json((*openapi).clone()) }
            })
        })
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn openapi_contains_every_public_operation_with_unique_ids() {
        let openapi = openapi_document();
        let expected_paths = [
            "/health",
            "/settings",
            "/projects",
            "/projects/{project_id}",
            "/projects/{project_id}/articles",
            "/projects/{project_id}/articles/{doi_key}",
            "/projects/{project_id}/graph",
            "/projects/{project_id}/recommendations",
            "/projects/{project_id}/metrics/recompute",
            "/ingestions",
            "/ingestions/{ingestion_id}",
            "/ingestions/{ingestion_id}/items",
            "/ingestions/{ingestion_id}/cancel",
        ];

        for path in expected_paths {
            assert!(
                openapi.paths.paths.contains_key(path),
                "missing path {path}"
            );
        }

        let mut operation_ids = HashSet::new();
        for path_item in openapi.paths.paths.values() {
            for operation in [
                &path_item.get,
                &path_item.put,
                &path_item.post,
                &path_item.delete,
                &path_item.options,
                &path_item.head,
                &path_item.patch,
                &path_item.trace,
            ]
            .into_iter()
            .flatten()
            {
                let operation_id = operation
                    .operation_id
                    .as_deref()
                    .expect("every operation must have an operationId");
                assert!(
                    operation_ids.insert(operation_id),
                    "duplicate operationId {operation_id}"
                );
            }
        }
    }

    #[tokio::test]
    async fn serves_the_exported_openapi_document() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/deepref")
            .expect("test database URL must be valid");
        let response = router(AppState { pool })
            .oneshot(
                Request::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .expect("request must be valid"),
            )
            .await
            .expect("OpenAPI request must succeed");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body must be readable");
        let served: serde_json::Value =
            serde_json::from_slice(&body).expect("response must contain valid OpenAPI JSON");
        let exported =
            serde_json::to_value(openapi_document()).expect("OpenAPI document must serialize");
        assert_eq!(served, exported);
    }
}
