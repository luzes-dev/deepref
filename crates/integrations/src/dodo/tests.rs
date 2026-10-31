#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use super::*;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    response::Response,
    routing::any,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

fn config() -> DodoConfig {
    deepref_config::RuntimeConfig::from_map("test",&HashMap::from([
            ("APP_ENV".into(), "development".into()),
        ("DODO_MODE".into(),"test_mode".into()),("DODO_BUSINESS_ID".into(),"bus_test".into()),("DODO_API_KEY".into(),"credential-never-log".into()),
        ("DODO_WEBHOOK_KEY".into(),"whsec_MDEyMzQ1Njc4OWFiY2RlZg==".into()),
        ("DODO_WEBHOOK_KEY_PREVIOUS".into(),"whsec_ZmVkY2JhOTg3NjU0MzIxMA==".into()),
        ("DODO_OFFERINGS_JSON".into(),r#"[{"offering":"primary","product_id":"pdt_reviewed","entitlements":{"features":["ai_first"],"projects":8,"members":2,"monthly_ai_units":0}}]"#.into())
    ])).unwrap().dodo.unwrap()
}
#[derive(Clone)]
struct Fixture {
    responses: Arc<Mutex<std::collections::VecDeque<(u16, String)>>>,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
}
async fn serve(State(f): State<Fixture>, request: Request) -> Response {
    assert_eq!(
        request.headers().get("authorization").unwrap(),
        "Bearer credential-never-log"
    );
    let path = request.uri().to_string();
    let body = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    f.requests.lock().unwrap().push((path, value));
    let (status, body) = f
        .responses
        .lock()
        .unwrap()
        .pop_front()
        .expect("unexpected provider request");
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from_stream(futures::stream::iter(
            body.as_bytes()
                .chunks(4096)
                .map(|chunk| {
                    Ok::<_, std::convert::Infallible>(axum::body::Bytes::copy_from_slice(chunk))
                })
                .collect::<Vec<_>>(),
        )))
        .unwrap()
}
async fn adapter(
    responses: Vec<(u16, Value)>,
) -> (DodoBilling, Fixture, tokio::task::JoinHandle<()>) {
    let f = Fixture {
        responses: Arc::new(Mutex::new(
            responses
                .into_iter()
                .map(|(s, b)| (s, b.to_string()))
                .collect(),
        )),
        requests: Default::default(),
    };
    let app = Router::new().fallback(any(serve)).with_state(f.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut dodo = DodoBilling::new(config()).unwrap();
    dodo.origin = origin;
    (dodo, f, task)
}
#[tokio::test]
async fn hosted_contract_is_closed_nullable_and_does_not_send_portal_email() {
    let (dodo, f, task) = adapter(vec![
        (
            200,
            json!({"checkout_url":"https://test.checkout.dodopayments.com/session/capability"}),
        ),
        (
            200,
            json!({"link":"https://test.customer.dodopayments.com/session/capability"}),
        ),
        (200, json!({"checkout_url":null})),
    ])
    .await;
    let c = CustomerReference::new("cus_test".into()).unwrap();
    assert!(
        dodo.checkout(&c, Offering::Primary, "/settings/billing")
            .await
            .is_ok()
    );
    assert!(dodo.portal(&c, "/settings").await.is_ok());
    assert!(matches!(
        dodo.checkout(&c, Offering::Primary, "/settings").await,
        Err(BillingError::Contract)
    ));
    assert!(matches!(
        dodo.checkout(&c, Offering::Primary, "//evil.test").await,
        Err(BillingError::Refused)
    ));
    let requests = f.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[0].1,
        json!({"customer":{"customer_id":"cus_test"},"product_cart":[{"product_id":"pdt_reviewed","quantity":1}],"return_url":"http://127.0.0.1:5173/settings/billing"})
    );
    assert!(requests[1].0.contains("send_email=false"));
    assert_eq!(requests[1].1, Value::Null);
    task.abort();
}
#[test]
fn hosted_links_reject_other_modes_credentials_fragments_and_unsafe_schemes() {
    let dodo = DodoBilling::new(config()).unwrap();
    for url in [
        "http://test.checkout.dodopayments.com/x",
        "https://checkout.dodopayments.com/x",
        "https://test.checkout.dodopayments.com.evil.test/x",
        "https://user@test.checkout.dodopayments.com/x",
        "https://test.checkout.dodopayments.com/x#secret",
        "https://test.customer.dodopayments.com/x",
    ] {
        assert!(dodo.hosted(Some(url.into()), false).is_err(), "{url}");
    }
}
#[tokio::test]
async fn unknown_creation_never_retries_post_and_zero_or_incomplete_scan_is_unresolved() {
    let (dodo, f, task) = adapter(vec![
        (503, json!({"error":"credential-never-log"})),
        (200, json!({"items":[]})),
    ])
    .await;
    let op = Uuid::new_v4();
    let contact =
        BillingContact::parse("owner@example.test".into(), "Billing owner".into()).unwrap();
    assert!(matches!(
        dodo.create_customer(CustomerIntent {
            workspace: Uuid::new_v4().into(),
            operation: op,
            contact: &contact
        })
        .await,
        Err(BillingError::Ambiguous)
    ));
    assert!(matches!(
        dodo.resolve_customer(op).await,
        Ok(CustomerResolution::Unresolved)
    ));
    let requests = f.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].1,
        json!({"email":"owner@example.test","name":"Billing owner","metadata":{"deepref_operation":op.to_string()}})
    );
    assert!(!requests[1].0.contains("email"));
    task.abort();
}
#[tokio::test]
async fn current_active_subscription_carries_adverse_payment_evidence() {
    let (dodo,_,task)=adapter(vec![(200,json!({"business_id":"bus_test","customer_id":"cus_test"})),(200,json!({"items":[{"subscription_id":"sub_fixture","customer":{"customer_id":"cus_test"},"product_id":"pdt_reviewed","status":"active"}]})),(200,json!({"items":[]})),(200,json!({"items":[{"customer":{"customer_id":"cus_test"},"payment_id":"pay_1","refund_status":"full","dispute_status":null}]})),(200,json!({"items":[]}))]).await;
    let result = dodo
        .observe(&CustomerReference::new("cus_test".into()).unwrap())
        .await
        .unwrap();
    assert_eq!(result.state, SubscriptionState::Active);
    assert!(result.review.is_some());
    task.abort();
}
#[test]
fn independent_python_standard_webhook_vectors_authenticate_raw_bytes_rotation_and_replay() {
    // Generated independently using Python stdlib hmac/sha256/base64, not this
    // verifier. Exact body includes whitespace; reserialization must fail.
    let dodo = DodoBilling::new(config()).unwrap();
    let body=br#"{ "business_id":"bus_test", "type":"subscription.active", "timestamp":"2026-10-24T00:00:00Z", "data":{"customer":{"customer_id":"cus_test"}} }"#;
    let now = chrono::DateTime::from_timestamp(1792800000, 0).unwrap();
    let verify = |b: &[u8], signature: &str, at| {
        dodo.verify(
            b,
            WebhookHeaders {
                id: "msg_vector",
                timestamp: "1792800000",
                signatures: signature,
            },
            at,
        )
    };
    assert!(verify(body, "v1,e9FSoXqpQb7bEcw2l7sZ3iEG48qMoUsW5+xO3uNjRJY=", now).is_ok());
    assert!(
        verify(
            body,
            "v1,invalid v1,6fx+8NaffAJBRtlmAioOqUEPg3/sk+EoLxfD+6CxONg=",
            now
        )
        .is_ok()
    );
    assert!(
        verify(
            br#"{}"#,
            "v1,e9FSoXqpQb7bEcw2l7sZ3iEG48qMoUsW5+xO3uNjRJY=",
            now
        )
        .is_err()
    );
    assert!(
        verify(
            body,
            "v1,e9FSoXqpQb7bEcw2l7sZ3iEG48qMoUsW5+xO3uNjRJY=",
            now + chrono::Duration::seconds(301)
        )
        .is_err()
    );
    let different = String::from_utf8(body.to_vec())
        .unwrap()
        .replace("bus_test", "bus_else");
    assert!(
        verify(
            different.as_bytes(),
            "v1,e9FSoXqpQb7bEcw2l7sZ3iEG48qMoUsW5+xO3uNjRJY=",
            now
        )
        .is_err()
    );
    assert!(
        matches!(
            verify(
                different.as_bytes(),
                "v1,DLlFjSQwGoO7RH7aovRELXGkXsUgU6apWFsXpCw4SdY=",
                now
            ),
            Err(BillingError::Refused)
        ),
        "a correctly signed foreign-business envelope must be refused"
    );
    let compact = serde_json::to_vec(&serde_json::from_slice::<Value>(body).unwrap()).unwrap();
    assert!(
        verify(
            &compact,
            "v1,e9FSoXqpQb7bEcw2l7sZ3iEG48qMoUsW5+xO3uNjRJY=",
            now
        )
        .is_err()
    );
}

#[tokio::test]
async fn bounded_customer_scan_never_claims_unique_match_from_incomplete_pages() {
    let customer = json!({"business_id":"bus_test","customer_id":"cus_unrelated","metadata":{}});
    let responses = (0..10)
        .map(|_| (200, json!({"items":[customer.clone()]})))
        .collect();
    let (dodo, f, task) = adapter(responses).await;
    assert!(matches!(
        dodo.resolve_customer(Uuid::new_v4()).await,
        Err(BillingError::Ambiguous)
    ));
    let requests = f.requests.lock().unwrap();
    assert_eq!(requests.len(), 10);
    assert!(requests[0].0.contains("page_number=1&"));
    assert!(requests[9].0.contains("page_number=10&"));
    task.abort();
}

#[test]
fn configured_scope_binds_mode_and_business_without_logging_provider_identity() {
    let first = DodoBilling::new(config()).unwrap();
    let mut live = config();
    live.mode = deepref_config::DodoMode::Live;
    let live = DodoBilling::new(live).unwrap();
    let mut other = config();
    other.business_id = "bus_another".into();
    let other = DodoBilling::new(other).unwrap();
    assert_ne!(first.namespace(), live.namespace());
    assert_ne!(first.namespace(), other.namespace());
    assert!(!first.namespace().contains("bus_test"));
}
#[test]
fn all_provider_lifecycle_states_are_closed_and_unknown_products_never_grant() {
    let c = CustomerReference::new("cus_test".into()).unwrap();
    for (raw, state) in [
        ("pending", SubscriptionState::Pending),
        ("on_hold", SubscriptionState::OnHold),
        ("paused", SubscriptionState::Paused),
        ("cancelled", SubscriptionState::Cancelled),
        ("failed", SubscriptionState::Failed),
        ("expired", SubscriptionState::Expired),
        ("past_due", SubscriptionState::PastDue),
        ("new_future_status", SubscriptionState::Unknown),
    ] {
        let o = observation(
            &config(),
            &c,
            vec![Subscription {
                subscription_id: "sub_fixture".into(),
                customer: LimitedCustomer {
                    customer_id: "cus_test".into(),
                },
                product_id: "pdt_reviewed".into(),
                status: raw.into(),
            }],
        )
        .unwrap();
        assert_eq!(o.state, state);
        assert!(o.entitlements.features.is_empty());
    }
    assert!(
        observation(
            &config(),
            &c,
            vec![Subscription {
                subscription_id: "sub_fixture".into(),
                customer: LimitedCustomer {
                    customer_id: "cus_test".into()
                },
                product_id: "pdt_unreviewed".into(),
                status: "active".into()
            }]
        )
        .is_err()
    );
}

#[test]
fn canonical_wire_payload_verification_is_strict_and_complete() {
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("amount".to_owned(), json!(42));
    metadata.insert("model".to_owned(), json!("test-model"));
    metadata.insert("provider".to_owned(), json!("test-provider"));

    let expected = DodoUsageEvent {
        customer_id: "cus_target".to_owned(),
        event_id: "evt_12345".to_owned(),
        event_name: "ai_commercial_units".to_owned(),
        timestamp: "2026-10-25T12:00:00Z".to_owned(),
        metadata: Some(metadata.clone()),
    };

    let record = DodoEventRecord {
        business_id: "bus_test".to_owned(),
        customer_id: "cus_target".to_owned(),
        event_id: "evt_12345".to_owned(),
        event_name: "ai_commercial_units".to_owned(),
        timestamp: "2026-10-25T12:00:00Z".to_owned(),
        metadata: Some(metadata.clone()),
    };

    // Identical record matches
    assert!(record.matches_canonical(&expected, "bus_test"));

    // Business ID mismatch
    assert!(!record.matches_canonical(&expected, "bus_other"));

    // Customer ID mismatch
    let mut rec_diff = record.clone();
    rec_diff.customer_id = "cus_other".to_owned();
    assert!(!rec_diff.matches_canonical(&expected, "bus_test"));

    // Event ID mismatch
    let mut rec_diff = record.clone();
    rec_diff.event_id = "evt_other".to_owned();
    assert!(!rec_diff.matches_canonical(&expected, "bus_test"));

    // Event name mismatch
    let mut rec_diff = record.clone();
    rec_diff.event_name = "other_units".to_owned();
    assert!(!rec_diff.matches_canonical(&expected, "bus_test"));

    // Timestamp mismatch
    let mut rec_diff = record.clone();
    rec_diff.timestamp = "2026-10-25T13:00:00Z".to_owned();
    assert!(!rec_diff.matches_canonical(&expected, "bus_test"));

    // Metadata amount mismatch
    let mut rec_diff = record.clone();
    let mut diff_meta = metadata.clone();
    diff_meta.insert("amount".to_owned(), json!(999));
    rec_diff.metadata = Some(diff_meta);
    assert!(!rec_diff.matches_canonical(&expected, "bus_test"));

    // Metadata extra or missing key mismatch
    let mut rec_diff = record.clone();
    let mut diff_meta = metadata.clone();
    diff_meta.insert("extra_key".to_owned(), json!("extra_val"));
    rec_diff.metadata = Some(diff_meta);
    assert!(!rec_diff.matches_canonical(&expected, "bus_test"));
}

#[tokio::test]
async fn ingest_usage_event_outcomes() {
    let (dodo, _, _task) = adapter(vec![
        (200, json!({"ingested_count": 1})),
        (200, json!({"ingested_count": 0})),
        (409, json!({"error": "conflict"})),
        (500, json!({"error": "internal"})),
        (400, json!({"error": "bad request"})),
    ])
    .await;

    let event = DodoUsageEvent {
        customer_id: "cus_test".to_owned(),
        event_id: "evt_test".to_owned(),
        event_name: "ai_commercial_units".to_owned(),
        timestamp: "2026-10-25T12:00:00Z".to_owned(),
        metadata: None,
    };

    // 200 + count 1 -> Accepted
    let outcome = dodo.ingest_usage_event(event.clone()).await.unwrap();
    assert_eq!(outcome, DodoIngestOutcome::Accepted);

    // 200 + count 0 -> Ambiguous
    let outcome = dodo.ingest_usage_event(event.clone()).await.unwrap();
    assert_eq!(outcome, DodoIngestOutcome::Ambiguous);

    // 409 -> Conflict
    let outcome = dodo.ingest_usage_event(event.clone()).await.unwrap();
    assert_eq!(outcome, DodoIngestOutcome::Conflict);

    // 500 -> Ambiguous
    let outcome = dodo.ingest_usage_event(event.clone()).await.unwrap();
    assert_eq!(outcome, DodoIngestOutcome::Ambiguous);

    // 400 -> Refused
    let outcome = dodo.ingest_usage_event(event.clone()).await.unwrap();
    assert!(matches!(outcome, DodoIngestOutcome::Refused(_)));
}

#[test]
fn canonical_usage_timestamp_requires_exact_instant_and_valid_rfc3339() {
    let mut expected = DodoUsageEvent {
        customer_id: "cus_timestamp".into(),
        event_id: "event_timestamp".into(),
        event_name: "units".into(),
        timestamp: "2026-10-25T12:00:00.123456Z".into(),
        metadata: None,
    };
    let mut record = DodoEventRecord {
        business_id: "business".into(),
        customer_id: expected.customer_id.clone(),
        event_id: expected.event_id.clone(),
        event_name: expected.event_name.clone(),
        timestamp: "2026-10-25T12:00:00.999999Z".into(),
        metadata: None,
    };
    assert!(
        !record.matches_canonical(&expected, "business"),
        "same second is not the same provider event instant"
    );
    record.timestamp = "2026-10-25T09:00:00.123456-03:00".into();
    assert!(
        record.matches_canonical(&expected, "business"),
        "equivalent timezone representation is the same instant"
    );
    record.timestamp = "invalid".into();
    expected.timestamp = "invalid".into();
    assert!(
        !record.matches_canonical(&expected, "business"),
        "matching invalid strings are not timestamp evidence"
    );
}

#[tokio::test]
async fn lookup_usage_event_outcomes() {
    let (dodo, _, _task) = adapter(vec![
        (
            200,
            json!({
                "business_id": "bus_test",
                "customer_id": "cus_test",
                "event_id": "evt_test",
                "event_name": "ai_commercial_units",
                "timestamp": "2026-10-25T12:00:00Z",
                "metadata": {"amount": 10}
            }),
        ),
        (404, json!({"error": "not found"})),
        (503, json!({"error": "service unavailable"})),
    ])
    .await;

    // 200 -> Some(record)
    let record = dodo.lookup_usage_event("evt_test").await.unwrap();
    assert!(record.is_some());
    assert_eq!(record.unwrap().event_id, "evt_test");

    // 404 -> None
    let record = dodo.lookup_usage_event("evt_missing").await.unwrap();
    assert!(record.is_none());

    // 503 -> BillingError::Unavailable
    let err = dodo.lookup_usage_event("evt_err").await.unwrap_err();
    assert_eq!(err, BillingError::Unavailable);
}

#[tokio::test]
async fn usage_ingest_rejects_oversize_unknown_length_response_as_ambiguous() {
    let (dodo, _, task) = adapter(vec![(
        200,
        json!({"ingested_count":1,"padding":"x".repeat(MAX_RESPONSE)}),
    )])
    .await;
    let event = DodoUsageEvent {
        customer_id: "cus_test".into(),
        event_id: "evt_test".into(),
        event_name: "units".into(),
        timestamp: "2026-10-25T12:00:00Z".into(),
        metadata: None,
    };
    assert_eq!(
        dodo.ingest_usage_event(event).await.unwrap(),
        DodoIngestOutcome::Ambiguous
    );
    task.abort();
}
#[tokio::test]
async fn usage_lookup_rejects_oversize_unknown_length_response() {
    let (dodo, _, task) = adapter(vec![(200, json!({"business_id":"bus_test","customer_id":"cus_test","event_id":"evt_test","event_name":"units","timestamp":"2026-10-25T12:00:00Z","metadata":{"padding":"x".repeat(MAX_RESPONSE)}}))]).await;
    assert_eq!(
        dodo.lookup_usage_event("evt_test").await.unwrap_err(),
        BillingError::Contract
    );
    task.abort();
}
#[tokio::test]
async fn usage_refusal_discards_provider_body_marker() {
    let marker = "PRIVATE_PROVIDER_CAPABILITY_SENTINEL";
    let (dodo, _, task) = adapter(vec![(400, json!({"error":marker}))]).await;
    let event = DodoUsageEvent {
        customer_id: "cus_test".into(),
        event_id: "evt_test".into(),
        event_name: "units".into(),
        timestamp: "2026-10-25T12:00:00Z".into(),
        metadata: None,
    };
    let outcome = dodo.ingest_usage_event(event).await.unwrap();
    assert!(
        !format!("{outcome:?}").contains(marker),
        "raw provider error must not cross the adapter boundary"
    );
    assert!(matches!(outcome, DodoIngestOutcome::Refused(_)));
    task.abort();
}

fn subscription(status: &str, customer: &str) -> Value {
    json!({"subscription_id":"sub_closure","customer":{"customer_id":customer},"product_id":"pdt_reviewed","status":status})
}

#[tokio::test]
async fn cancel_subscription_reads_first_patches_once_and_never_claims_on_refusal() {
    let (dodo, f, _task) = adapter(vec![
        // Active: read, then PATCH reporting cancelled.
        (200, subscription("active", "cus_closure")),
        (200, subscription("cancelled", "cus_closure")),
        // Already ended: read only, no second PATCH.
        (200, subscription("expired", "cus_closure")),
        // Foreign customer: manual, no PATCH.
        (200, subscription("active", "cus_other")),
        // Provider refuses the PATCH.
        (200, subscription("active", "cus_closure")),
        (422, json!({"error":"x"})),
        // Outage: retryable error, not a recorded outcome.
        (503, json!({})),
        // 2xx without the ended state proves nothing.
        (200, subscription("active", "cus_closure")),
        (200, subscription("active", "cus_closure")),
    ])
    .await;
    let c = CustomerReference::new("cus_closure".into()).unwrap();
    let s = CustomerReference::new("sub_closure".into()).unwrap();
    assert_eq!(
        dodo.cancel_subscription(&c, &s).await.unwrap(),
        SubscriptionCancellation::Cancelled
    );
    assert_eq!(
        dodo.cancel_subscription(&c, &s).await.unwrap(),
        SubscriptionCancellation::Cancelled
    );
    assert_eq!(
        dodo.cancel_subscription(&c, &s).await.unwrap(),
        SubscriptionCancellation::ManualRequired
    );
    assert_eq!(
        dodo.cancel_subscription(&c, &s).await.unwrap(),
        SubscriptionCancellation::ManualRequired
    );
    assert_eq!(
        dodo.cancel_subscription(&c, &s).await,
        Err(BillingError::Unavailable)
    );
    assert_eq!(
        dodo.cancel_subscription(&c, &s).await,
        Err(BillingError::Unavailable)
    );
    let requests = f.requests.lock().unwrap();
    let patches: Vec<_> = requests
        .iter()
        .filter(|(_, body)| !body.is_null())
        .collect();
    assert_eq!(patches.len(), 3);
    assert!(
        patches
            .iter()
            .all(|(path, body)| path == "/subscriptions/sub_closure"
                && *body == json!({"status":"cancelled"}))
    );
}
