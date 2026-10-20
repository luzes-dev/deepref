#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Authentication rejections are audited and bounded (handoff 11 Part B).
//! Attacker-controlled floods — bad cookies, replayed callbacks, cross-origin
//! posts — must not saturate audit storage, and nothing caller-supplied is
//! ever stored.

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use deepref_app::auth_rejections::{AuthRejection, RejectionAuditor, RejectionReason};
use sqlx::{PgPool, Row};
use tower::ServiceExt;
use uuid::Uuid;

mod support;
use support::SESSION_COOKIE;
use support::authz::Harness;

async fn rejection_rows(pool: &PgPool) -> Vec<serde_json::Value> {
    sqlx::query("SELECT row_to_json(a)::text AS row FROM admin_audit_events a WHERE event_type = 'auth.rejected'")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| serde_json::from_str(&row.get::<String, _>("row")).unwrap())
        .collect()
}

async fn rejection_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM admin_audit_events WHERE event_type = 'auth.rejected'")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_flood_of_authentication_rejections_stores_a_bounded_number_of_rows() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let person = h.sign_in("user_A", "a@example.org").await;

    for i in 0..250 {
        // Not even a UUID: the malformed-cookie rejection.
        let response = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/me")
                    .header(
                        header::COOKIE,
                        format!("{SESSION_COOKIE}=COOKIE-SECRET-{i}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        // A well-formed but unknown cookie.
        let response = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/me")
                    .header(
                        header::COOKIE,
                        format!("{SESSION_COOKIE}={}", Uuid::new_v4()),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        // A replayed callback with attacker-chosen values.
        let response = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/callback?code=CALLBACK-CODE-SECRET&state=CALLBACK-STATE-SECRET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        // A cross-origin cookie-authenticated post.
        let response = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/me/deletion-request")
                    .header(
                        header::COOKIE,
                        format!("{SESSION_COOKIE}={}", person.cookie),
                    )
                    .header(header::ORIGIN, "https://evil.example")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    let rows = rejection_rows(&h.pool).await;
    // Four (stage, reason) keys in one one-minute window: a flood stores a
    // handful of rows, not 1000.
    assert!(
        rows.len() <= 8,
        "a 1000-request flood stored {} rows",
        rows.len()
    );
    assert!(rows.len() >= 4, "every rejection kind is recorded once");
    for row in &rows {
        let metadata = &row["metadata"];
        let mut keys = metadata
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(keys, vec!["reason", "stage", "window"]);
        assert!(metadata["window"].as_str().unwrap().parse::<u64>().is_ok());
        assert!(row["workspace_id"].is_null());
        assert!(row["project_id"].is_null());
        assert_eq!(row["principal_kind"], "system");
        assert!(row["principal_user_id"].is_null());
    }

    // Nothing a caller supplied is anywhere in the rows.
    let serialized = serde_json::to_string(&rows).unwrap();
    for secret in [
        "COOKIE-SECRET-",
        "CALLBACK-CODE-SECRET",
        "CALLBACK-STATE-SECRET",
    ] {
        assert!(!serialized.contains(secret), "{secret} reached the audit");
    }
}

#[tokio::test]
async fn a_rejection_names_the_user_only_from_a_stored_session() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let person = h.sign_in("user_A", "a@example.org").await;

    // A tampered stored session is refused and names the user it belonged to.
    sqlx::query(
        "UPDATE user_sessions SET sealed_provider_session = 'sealed-tampered' WHERE user_id = $1",
    )
    .bind(person.user.as_uuid())
    .execute(&h.pool)
    .await
    .unwrap();
    let (status, _) = h.get(&person, "/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let rows = rejection_rows(&h.pool).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["metadata"]["stage"], "session");
    assert_eq!(rows[0]["metadata"]["reason"], "invalid_session");
    assert_eq!(rows[0]["target_user_id"], person.user.as_uuid().to_string());

    // An unknown cookie stores no target at all.
    let response = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/me")
                .header(
                    header::COOKIE,
                    format!("{SESSION_COOKIE}={}", Uuid::new_v4()),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let rows = rejection_rows(&h.pool).await;
    let anonymous: Vec<_> = rows
        .iter()
        .filter(|row| row["target_user_id"].is_null())
        .collect();
    assert_eq!(anonymous.len(), 1, "{rows:?}");
    assert_eq!(
        anonymous[0]["metadata"]["reason"],
        "unknown_expired_or_revoked_session"
    );
}

#[tokio::test]
async fn a_terminal_refresh_failure_is_audited_as_a_refresh_rejection() {
    let Some(h) = Harness::new().await else {
        return;
    };
    let person = h.sign_in("user_A", "a@example.org").await;

    // The provider has already spent this session's refresh token.
    let spent = h.fake.mint_revoked_session("user_A");
    sqlx::query("UPDATE user_sessions SET sealed_provider_session = $2 WHERE user_id = $1")
        .bind(person.user.as_uuid())
        .bind(spent)
        .execute(&h.pool)
        .await
        .unwrap();
    let (status, _) = h.get(&person, "/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let rows = rejection_rows(&h.pool).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["metadata"]["stage"], "refresh");
    assert_eq!(rows[0]["metadata"]["reason"], "refresh_rejected");
    assert_eq!(rows[0]["target_user_id"], person.user.as_uuid().to_string());
}

#[tokio::test]
async fn the_rejection_bound_holds_across_processes() {
    let Some(pool) = support::authz::database().await else {
        return;
    };
    let first = RejectionAuditor::new(pool.clone());
    let second = RejectionAuditor::new(pool.clone());
    let rejection = AuthRejection {
        reason: RejectionReason::SessionMalformedCookie,
        target: None,
        request_id: Some("00000000-0000-4000-8000-000000000077"),
    };
    assert!(first.record(rejection).await.unwrap());
    assert!(
        !second.record(rejection).await.unwrap(),
        "the unique index bounds the window across processes"
    );
    assert_eq!(rejection_count(&pool).await, 1);
}

#[tokio::test]
async fn no_forbidden_secret_pattern_reaches_the_audit() {
    let Some(h) = Harness::new().await else {
        return;
    };
    // Flows that touch the values listed in 11 ("Never store"): the login
    // cookie and its one-time state, the provider code, the session cookie
    // and its sealed provider session, and caller-chosen query text.
    let person = h.sign_in("user_A", "a@example.org").await;
    let (status, _) = h
        .send(&person, Method::POST, "/me/deletion-request", Some("{}"))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    for attempt in [
        "/auth/callback?code=FORBIDDEN-CODE-VALUE&state=FORBIDDEN-STATE-VALUE",
        "/auth/callback?code=FORBIDDEN-CODE-VALUE&state=FORBIDDEN-STATE-VALUE&error=access_denied",
    ] {
        h.app
            .clone()
            .oneshot(Request::builder().uri(attempt).body(Body::empty()).unwrap())
            .await
            .unwrap();
    }
    for secret_cookie in ["FORBIDDEN-COOKIE-VALUE", "FORBIDDEN-COOKIE-VALUE-TWO"] {
        h.app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/me")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}={secret_cookie}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    let (status, _) = h
        .send(&person, Method::POST, "/auth/logout", Some("{}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The scan: every audit row, every forbidden pattern.
    let rows: Vec<String> =
        sqlx::query("SELECT row_to_json(a)::text AS row FROM admin_audit_events a")
            .fetch_all(&h.pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("row"))
            .collect();
    assert!(!rows.is_empty(), "the flows wrote audit rows to scan");
    for row in &rows {
        for pattern in [
            // values the flows deliberately placed in cookies and queries
            "FORBIDDEN-",
            // the real credentials the flows used
            person.cookie.as_str(),
            // forbidden shapes
            "Bearer ",
            "eyJ",
            "Set-Cookie",
            "Authorization",
            "sealed",
            "session_token",
        ] {
            assert!(!row.contains(pattern), "{pattern} reached the audit: {row}");
        }
    }
}

#[tokio::test]
async fn the_database_refuses_an_unbounded_rejection_shape() {
    let Some(pool) = support::authz::database().await else {
        return;
    };
    let insert = |metadata: serde_json::Value, kind: &str| {
        let pool = pool.clone();
        let kind = kind.to_owned();
        async move {
            sqlx::query(
                "INSERT INTO admin_audit_events
                   (id, event_type, principal_kind, principal_user_id, target_user_id, metadata)
                 VALUES ($1, 'auth.rejected', $2, NULL, NULL, $3)",
            )
            .bind(Uuid::new_v4())
            .bind(kind)
            .bind(metadata)
            .execute(&pool)
            .await
        }
    };

    // The shape is closed: three keys, closed vocabularies, numeric window.
    for metadata in [
        serde_json::json!({"stage": "session", "reason": "invalid_session", "window": "1", "extra": "x"}),
        serde_json::json!({"stage": "session", "reason": "invalid_state", "window": "1"}),
        serde_json::json!({"stage": "callback", "reason": "invalid_session", "window": "1"}),
        serde_json::json!({"stage": "session", "reason": "invalid_session", "window": "soon"}),
        serde_json::json!({"stage": "session", "reason": "invalid_session"}),
    ] {
        assert!(
            insert(metadata, "system").await.is_err(),
            "the database refused an unbounded shape"
        );
    }
    // The row is a system row: no principal to name before authentication.
    assert!(
        insert(
            serde_json::json!({"stage": "session", "reason": "invalid_session", "window": "1"}),
            "user"
        )
        .await
        .is_err()
    );

    // One row per (stage, reason, target, window) is the most the table holds.
    let metadata =
        serde_json::json!({"stage": "origin", "reason": "origin_mismatch", "window": "1"});
    insert(metadata.clone(), "system").await.unwrap();
    assert!(
        insert(metadata, "system").await.is_err(),
        "the unique index refuses the second row of a window"
    );
}
