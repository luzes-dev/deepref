#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! The administrative audit never reaches a research export (handoff 11 Part
//! B): no export kind includes `admin_audit_events` data, and no export module
//! reads the table at all.

use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

/// A value no export may ever contain, planted in the administrative audit.
const SENTINEL: &str = "SENTINEL-ADMIN-AUDIT-EXCLUSION";

const EXPORT_KINDS: [&str; 11] = [
    "reports.csv",
    "reports.json",
    "reports.ris",
    "reports.bib",
    "extraction.csv",
    "appraisal.csv",
    "included_studies.csv",
    "prisma.json",
    "prisma.svg",
    "audit.csv",
    "protocol.json",
];

/// Every administrative event type the audit may hold (migration 0109).
const ADMIN_EVENT_TYPES: [&str; 28] = [
    "auth.login",
    "auth.logout",
    "auth.rejected",
    "auth.session_revoked",
    "user.provisioned",
    "user.deactivated",
    "workspace.created",
    "workspace.renamed",
    "workspace.deleted",
    "membership.created",
    "membership.role_changed",
    "membership.status_changed",
    "membership.removed",
    "project_access.granted",
    "project_access.role_changed",
    "project_access.removed",
    "project.created",
    "project.deleted",
    "invitation.created",
    "invitation.accepted",
    "invitation.revoked",
    "invitation.expired",
    "authorization.denied",
    "legacy_actor.mapped",
    "legacy_actor.non_identity_recorded",
    "account.deletion_requested",
    "account.deletion_cancelled",
    "account.deletion_completed",
];

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("TEST_DATABASE_URL", 5).await
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-exports-test",
        &HashMap::from([("APP_ENV".to_owned(), "development".to_owned())]),
    )
    .expect("local test runtime should parse");
    ApiConfig {
        documents: deepref_config::DocumentConfig::from_source(&|name| {
            (name == "APP_ENV").then(|| "development".into())
        })
        .expect("local test document configuration"),
        runtime,
        bind_addr: "127.0.0.1:0".parse().expect("test bind address is valid"),
        cors_allow_any: false,
        cors_origins: Vec::new(),
    }
}

async fn request(pool: &PgPool, uri: String) -> Response<Body> {
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("export request should be valid"),
        )
        .await
        .expect("export request should be handled")
}

async fn response_bytes(response: Response<Body>) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("export body should be readable")
        .to_vec()
}

/// Fills the administrative audit with one row of every event type, each
/// carrying the sentinel in its request id.
async fn plant_admin_audit(pool: &PgPool) {
    let user_a = Uuid::new_v4();
    for user in [user_a] {
        sqlx::query(
            "INSERT INTO users (id, workos_user_id, primary_email, status)
             VALUES ($1, $2, 'export@example.org', 'active')",
        )
        .bind(user)
        .bind(format!("user_{user}"))
        .execute(pool)
        .await
        .unwrap();
    }
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    for event in ADMIN_EVENT_TYPES {
        let (metadata, workspace_id, project_id, target): (
            serde_json::Value,
            Option<Uuid>,
            Option<Uuid>,
            Option<Uuid>,
        ) = match event {
            "user.deactivated" => (
                serde_json::json!({"source": "account_deletion"}),
                None,
                None,
                Some(user_a),
            ),
            "auth.session_revoked" => (
                serde_json::json!({"reason": "user_deactivated"}),
                None,
                None,
                Some(user_a),
            ),
            "workspace.created" => (
                serde_json::json!({"workspace_kind": "personal"}),
                Some(workspace),
                None,
                None,
            ),
            "workspace.renamed" | "workspace.deleted" => {
                (serde_json::json!({}), Some(workspace), None, None)
            }
            "membership.created" | "membership.removed" => (
                serde_json::json!({"role": "member", "grant_source": "local"}),
                Some(workspace),
                None,
                Some(user_a),
            ),
            "membership.role_changed" => (
                serde_json::json!({"role": "member"}),
                Some(workspace),
                None,
                Some(user_a),
            ),
            "membership.status_changed" => (
                serde_json::json!({"status": "active"}),
                Some(workspace),
                None,
                Some(user_a),
            ),
            "project_access.granted" | "project_access.removed" => (
                serde_json::json!({"role": "member", "grant_source": "local"}),
                Some(workspace),
                Some(project),
                Some(user_a),
            ),
            "project_access.role_changed" => (
                serde_json::json!({"role": "member"}),
                Some(workspace),
                Some(project),
                Some(user_a),
            ),
            "project.created" | "project.deleted" => {
                (serde_json::json!({}), Some(workspace), Some(project), None)
            }
            "invitation.created"
            | "invitation.accepted"
            | "invitation.revoked"
            | "invitation.expired" => (
                serde_json::json!({"role": "member", "invitation_id": Uuid::new_v4().to_string()}),
                Some(workspace),
                None,
                (event == "invitation.accepted").then_some(user_a),
            ),
            "authorization.denied" => (
                serde_json::json!({"permission": "project:view", "resource_type": "none", "window": "1"}),
                None,
                None,
                None,
            ),
            "legacy_actor.mapped" | "legacy_actor.non_identity_recorded" => (
                serde_json::json!({"legacy_actor_kind": "user", "legacy_actor_id": "someone", "source": "test"}),
                None,
                None,
                Some(user_a),
            ),
            "account.deletion_requested" | "account.deletion_cancelled" => {
                (serde_json::json!({}), None, None, Some(user_a))
            }
            "account.deletion_completed" => (serde_json::json!({}), None, None, Some(user_a)),
            "auth.rejected" => (
                serde_json::json!({"stage": "session", "reason": "invalid_session", "window": "1"}),
                None,
                None,
                None,
            ),
            "audit.purged" => (serde_json::json!({"rows_purged": 0}), None, None, None),
            _ => (serde_json::json!({}), None, None, None),
        };
        // A few shapes pin the principal: denials and legacy-actor decisions
        // are written by a user, and a deletion request names itself.
        let principal = match event {
            "authorization.denied"
            | "legacy_actor.mapped"
            | "legacy_actor.non_identity_recorded"
            | "account.deletion_requested"
            | "account.deletion_cancelled" => Some(user_a),
            _ => None,
        };
        let target = match event {
            "account.deletion_requested" | "account.deletion_cancelled" => Some(user_a),
            _ => target,
        };
        sqlx::query(
            "INSERT INTO admin_audit_events
               (id, event_type, principal_kind, principal_user_id, target_user_id,
                workspace_id, project_id, request_id, metadata)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(Uuid::new_v4())
        .bind(event)
        .bind(if principal.is_some() {
            "user"
        } else {
            "system"
        })
        .bind(principal)
        .bind(target)
        .bind(workspace_id)
        .bind(project_id)
        .bind(SENTINEL)
        .bind(metadata)
        .execute(pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn no_research_export_carries_administrative_audit_data() {
    let Some(pool) = database().await else {
        return;
    };
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name) VALUES ($1, 'export boundary')")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    plant_admin_audit(&pool).await;

    for kind in EXPORT_KINDS {
        let response = request(&pool, format!("/projects/{project}/exports/{kind}")).await;
        let body = response_bytes(response).await;
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains(SENTINEL),
            "{kind} carries administrative audit data"
        );
        for event in ADMIN_EVENT_TYPES {
            assert!(
                !text.contains(event),
                "{kind} carries the administrative event type {event}"
            );
        }
    }

    // The project-scoped audit export names only project-scoped aggregates.
    let response = request(&pool, format!("/projects/{project}/exports/audit.csv")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let text = String::from_utf8_lossy(&response_bytes(response).await).to_string();
    for line in text.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let aggregate = line.split(',').next().unwrap_or_default();
        assert!(
            !aggregate.contains("admin")
                && !aggregate.contains("auth")
                && !aggregate.contains("account"),
            "audit.csv row looks administrative: {line}"
        );
    }
}

#[test]
fn no_export_module_reads_the_administrative_audit() {
    for path in [
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/routes/exports.rs"),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/routes/exports/bibliography.rs"
        ),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/routes/exports/csv_output.rs"
        ),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/routes/exports/review_tables.rs"
        ),
    ] {
        let source = std::fs::read_to_string(path).expect("export source should be readable");
        assert!(
            !source.contains("admin_audit_events"),
            "{path} reads the administrative audit"
        );
    }
    // The project-scoped audit export builds its rows from the scientific
    // event tables only.
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../app/src/audit_export.rs"
    ))
    .expect("audit export source should be readable");
    assert!(!source.contains("admin_audit_events"));
}
