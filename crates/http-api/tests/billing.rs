#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::{DateTime, Utc};
use deepref_application::billing::*;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;
mod support;
use support::authz::Harness;
struct Fake;
impl BillingProvider for Fake {
    fn namespace(&self) -> &str {
        "dodo:test_mode:bus_http"
    }
    fn create_customer<'a>(
        &'a self,
        _: CustomerIntent<'a>,
    ) -> BillingFuture<'a, CustomerReference> {
        Box::pin(async { CustomerReference::new("cus_http".into()) })
    }
    fn resolve_customer<'a>(&'a self, _: Uuid) -> BillingFuture<'a, CustomerResolution> {
        Box::pin(async { Ok(CustomerResolution::Unresolved) })
    }
    fn checkout<'a>(
        &'a self,
        _: &'a CustomerReference,
        _: Offering,
        _: &'a str,
    ) -> BillingFuture<'a, HostedLink> {
        Box::pin(async {
            Ok(HostedLink::validated(
                "https://test.checkout.dodopayments.com/session/secret".into(),
            ))
        })
    }
    fn portal<'a>(&'a self, c: &'a CustomerReference, p: &'a str) -> BillingFuture<'a, HostedLink> {
        self.checkout(c, Offering::Primary, p)
    }
    fn observe<'a>(
        &'a self,
        _: &'a CustomerReference,
    ) -> BillingFuture<'a, SubscriptionObservation> {
        Box::pin(async { Err(BillingError::Unavailable) })
    }
    fn verify(
        &self,
        _: &[u8],
        _: WebhookHeaders<'_>,
        _: DateTime<Utc>,
    ) -> Result<VerifiedBillingWebhook, BillingError> {
        Err(BillingError::Refused)
    }
    fn cancel_subscription<'a>(
        &'a self,
        _: &'a CustomerReference,
        _: &'a CustomerReference,
    ) -> BillingFuture<'a, SubscriptionCancellation> {
        Box::pin(async { Err(BillingError::Unavailable) })
    }
}
#[tokio::test]
async fn human_management_is_tenant_scoped() {
    let Some(pool) = support::authz::database().await else {
        return;
    };
    let h = Harness::with_state(pool, |mut state| {
        state.billing = Some(deepref_app::billing::BillingService::new(
            state.pool.clone(),
            Arc::new(Fake),
        ));
        state
    });
    let owner = h.sign_in("billing_owner", "owner@example.test").await;
    let foreign = h.sign_in("billing_foreign", "foreign@example.test").await;
    for (suffix, body) in [
        (
            "customer",
            json!({"email":"owner@example.test","name":"Owner"}),
        ),
        (
            "checkout",
            json!({"offering":"primary","return_path":"/settings"}),
        ),
        ("portal", json!({"return_path":"/settings"})),
    ] {
        let url = format!("/workspaces/{}/billing/{suffix}", owner.workspace.as_uuid());
        let missing = format!("/workspaces/{}/billing/{suffix}", Uuid::new_v4());
        let actual = h
            .send(&foreign, Method::POST, &url, Some(&body.to_string()))
            .await;
        let absent = h
            .send(&foreign, Method::POST, &missing, Some(&body.to_string()))
            .await;
        assert_eq!(actual.0, StatusCode::NOT_FOUND);
        assert_eq!(actual, absent);
    }
    let path = format!("/workspaces/{}/billing/customer", owner.workspace.as_uuid());
    assert_eq!(
        h.send(
            &owner,
            Method::POST,
            &path,
            Some(r#"{"email":"owner@example.test","name":"Owner"}"#)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // API keys cannot claim a human session, even with a valid workspace grant.
    let (status, key) = h
        .json(
            &owner,
            Method::POST,
            &format!("/workspaces/{}/api-keys", owner.workspace.as_uuid()),
            r#"{"name":"read","scopes":["workspace:view"]}"#,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    for suffix in ["customer", "checkout", "portal"] {
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!(
                "/workspaces/{}/billing/{suffix}",
                owner.workspace.as_uuid()
            ))
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", key["secret"].as_str().unwrap()),
            )
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            h.app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }
    let linkpath = format!("/workspaces/{}/billing/checkout", owner.workspace.as_uuid());
    let request = Request::builder()
        .method(Method::POST)
        .uri(linkpath)
        .header(
            header::COOKIE,
            format!("{}={}", support::SESSION_COOKIE, owner.cookie),
        )
        .header(header::ORIGIN, support::ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"offering":"primary","return_path":"/settings"}"#,
        ))
        .unwrap();
    let response = h.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::PRAGMA], "no-cache");
    let response = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/hooks/billing/dodo")
                .header("webhook-id", "evt")
                .header("webhook-timestamp", "0")
                .header("webhook-signature", "invalid")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM integration_inbox_events")
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        0
    );
}
