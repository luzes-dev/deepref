mod acquisitions;
mod articles;
mod deduplication;
mod health;
mod ingestions;
mod pagination;
mod projection;
mod projects;
mod review;
mod settings;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::{
    Json, Router,
    extract::Request,
    http::{Method, header},
    middleware::{Next, from_fn},
    response::Response,
    routing::get,
};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use utoipa::openapi::{Info, OpenApi};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{config::ApiConfig, state::AppState};

fn openapi_router() -> OpenApiRouter<AppState> {
    let mut openapi = OpenApi::default();
    openapi.info = Info::new("DeepRef API", env!("CARGO_PKG_VERSION"));

    OpenApiRouter::with_openapi(openapi)
        .routes(routes!(health::live))
        .routes(routes!(health::ready))
        .routes(routes!(health::dependencies))
        .routes(routes!(health::deprecated_health))
        .routes(routes!(projection::get_projection))
        .routes(routes!(settings::get_settings, settings::update_settings))
        .routes(routes!(projects::list_projects, projects::create_project))
        .routes(routes!(
            projects::get_project,
            projects::update_project,
            projects::delete_project
        ))
        .routes(routes!(articles::list_reports))
        .routes(routes!(articles::get_report))
        .routes(routes!(articles::project_graph))
        .routes(routes!(articles::recommendations))
        .routes(routes!(articles::recompute_metrics))
        .routes(routes!(
            acquisitions::list_acquisitions,
            acquisitions::create_acquisition
        ))
        .routes(routes!(acquisitions::import_project_records))
        .routes(routes!(deduplication::run_project_deduplication))
        .routes(routes!(deduplication::list_project_dedupe_proposals))
        .routes(routes!(deduplication::decide_project_dedupe_proposal))
        .routes(routes!(deduplication::resolve_project_record))
        .routes(routes!(
            ingestions::list_ingestions,
            ingestions::create_ingestion
        ))
        .routes(routes!(ingestions::get_ingestion))
        .routes(routes!(ingestions::list_ingestion_items))
        .routes(routes!(ingestions::cancel_ingestion))
        .routes(routes!(review::get_protocol))
        .routes(routes!(review::list_title_abstract_queue))
        .routes(routes!(review::screen_report))
        .routes(routes!(review::get_prisma))
}

pub fn openapi_document() -> OpenApi {
    openapi_router().into_openapi()
}

pub fn router(state: AppState, config: &ApiConfig) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(if config.cors_allow_any {
            tower_http::cors::AllowOrigin::any()
        } else {
            tower_http::cors::AllowOrigin::list(config.cors_origins.clone())
        })
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers([
            header::CONTENT_TYPE,
            header::HeaderName::from_static("idempotency-key"),
        ]);

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
        .layer(DefaultBodyLimit::max(acquisitions::MAX_REQUEST_BODY_BYTES))
        .layer(from_fn(correlation))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn correlation(mut request: Request, next: Next) -> Response {
    const HEADER: &str = "x-correlation-id";
    let correlation_id = request
        .headers()
        .get(HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .unwrap_or_else(uuid::Uuid::new_v4);
    request.extensions_mut().insert(correlation_id);
    let mut response = next.run(request).await;
    if let Ok(value) = correlation_id.to_string().parse() {
        response.headers_mut().insert(HEADER, value);
    }
    response
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
            "/health/live",
            "/health/ready",
            "/health/dependencies",
            "/projects/{project_id}/projection",
            "/settings",
            "/projects",
            "/projects/{project_id}",
            "/projects/{project_id}/reports",
            "/projects/{project_id}/reports/{report_id}",
            "/projects/{project_id}/graph",
            "/projects/{project_id}/recommendations",
            "/projects/{project_id}/metrics/recompute",
            "/projects/{project_id}/acquisitions",
            "/projects/{project_id}/imports",
            "/projects/{project_id}/deduplication/run",
            "/projects/{project_id}/deduplication/proposals",
            "/projects/{project_id}/deduplication/proposals/{proposal_id}/decision",
            "/projects/{project_id}/records/{record_id}/resolution",
            "/ingestions",
            "/ingestions/{ingestion_id}",
            "/ingestions/{ingestion_id}/items",
            "/ingestions/{ingestion_id}/cancel",
            "/projects/{project_id}/protocol",
            "/projects/{project_id}/screening/title-abstract",
            "/projects/{project_id}/reports/{report_id}/screening",
            "/projects/{project_id}/prisma",
        ];

        for path in expected_paths {
            assert!(
                openapi.paths.paths.contains_key(path),
                "missing path {path}"
            );
        }
        assert_eq!(
            openapi.paths.paths["/projects/{project_id}/reports"]
                .get
                .as_ref()
                .and_then(|operation| operation.operation_id.as_deref()),
            Some("listProjectReports")
        );
        assert_eq!(
            openapi.paths.paths["/projects/{project_id}/reports/{report_id}"]
                .get
                .as_ref()
                .and_then(|operation| operation.operation_id.as_deref()),
            Some("getProjectReport")
        );
        assert!(
            !openapi
                .paths
                .paths
                .contains_key("/projects/{project_id}/articles")
        );

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

        let document = serde_json::to_value(&openapi).expect("OpenAPI document must serialize");
        let schemas = &document["components"]["schemas"];
        for schema_name in [
            "ReportDto",
            "ReportDetailDto",
            "PaginatedResponse_ReportDto",
            "ProjectGraphDto",
            "RecommendationGroupsDto",
        ] {
            let schema = &schemas[schema_name];
            assert!(schema.is_object(), "missing public schema {schema_name}");
            assert!(
                schema.to_string().contains("report_id")
                    || schema
                        .to_string()
                        .contains("#/components/schemas/ReportDto"),
                "public schema {schema_name} must expose or reference report_id"
            );
            assert!(
                !schema.to_string().contains("doi_key"),
                "public schema {schema_name} leaks DOI identity"
            );
        }
        for schema_name in ["ReportDto", "ReportDetailDto"] {
            assert!(
                schemas[schema_name]["properties"]["report_id"].is_object(),
                "public schema {schema_name} must expose report_id"
            );
        }
        assert_eq!(
            schemas["GraphEdgeDto"]["properties"]["source"]["format"],
            "uuid"
        );
        assert_eq!(
            schemas["GraphEdgeDto"]["properties"]["target"]["format"],
            "uuid"
        );
    }

    #[test]
    fn import_body_limit_allows_bounded_json_string_escaping() {
        const {
            assert!(
                acquisitions::MAX_REQUEST_BODY_BYTES
                    >= acquisitions::MAX_IMPORT_BYTES * 2 + 64 * 1024
            );
            assert!(acquisitions::MAX_REQUEST_BODY_BYTES <= 16 * 1024 * 1024);
        }
    }

    #[test]
    fn acquisition_post_responses_match_handler_statuses() {
        let document =
            serde_json::to_value(openapi_document()).expect("OpenAPI document must serialize");
        let cases: [(&str, &[&str]); 2] = [
            (
                "/projects/{project_id}/acquisitions",
                &["200", "201", "400", "404", "409", "500"],
            ),
            (
                "/projects/{project_id}/imports",
                &["200", "201", "400", "404", "409", "413", "500"],
            ),
        ];

        for (path, expected) in cases {
            let responses = document["paths"][path]["post"]["responses"]
                .as_object()
                .unwrap_or_else(|| panic!("missing POST responses for {path}"));
            let actual: HashSet<_> = responses.keys().map(String::as_str).collect();
            let expected: HashSet<_> = expected.iter().copied().collect();
            assert_eq!(
                actual, expected,
                "unexpected POST response contract for {path}"
            );
        }
    }

    #[test]
    fn deduplication_response_contracts_match_handler_statuses() {
        let document =
            serde_json::to_value(openapi_document()).expect("OpenAPI document must serialize");
        let cases: [(&str, &str, &[&str]); 4] = [
            (
                "/projects/{project_id}/deduplication/run",
                "post",
                &["200", "400", "404", "500"],
            ),
            (
                "/projects/{project_id}/deduplication/proposals",
                "get",
                &["200", "400", "404", "500"],
            ),
            (
                "/projects/{project_id}/deduplication/proposals/{proposal_id}/decision",
                "post",
                &["200", "400", "404", "409", "500"],
            ),
            (
                "/projects/{project_id}/records/{record_id}/resolution",
                "post",
                &["200", "400", "404", "409", "500"],
            ),
        ];
        for (path, method, expected) in cases {
            let responses = document["paths"][path][method]["responses"]
                .as_object()
                .unwrap_or_else(|| panic!("missing {method} responses for {path}"));
            let actual: HashSet<_> = responses.keys().map(String::as_str).collect();
            let expected: HashSet<_> = expected.iter().copied().collect();
            assert_eq!(
                actual, expected,
                "unexpected response contract for {method} {path}"
            );
        }
    }

    #[tokio::test]
    async fn serves_the_exported_openapi_document() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/deepref")
            .expect("test database URL must be valid");
        let mut values = std::collections::HashMap::new();
        values.insert("APP_ENV".to_owned(), "local".to_owned());
        let runtime = deepref_config::RuntimeConfig::from_map("test", &values).unwrap();
        let config = ApiConfig {
            runtime,
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            cors_allow_any: false,
            cors_origins: vec!["http://localhost:3000".parse().unwrap()],
        };
        let response = router(AppState::core(pool), &config)
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
