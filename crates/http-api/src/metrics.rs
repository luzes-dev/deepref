//! HTTP metric labels are closed categories; even matched route templates do
//! not leave this adapter, and arbitrary request paths never become labels.
use std::time::Instant;

use axum::{
    extract::{MatchedPath, Request, State},
    http::{Method, header},
    middleware::Next,
    response::Response,
};
use deepref_telemetry::metrics::{HttpMethod, HttpMetrics, RouteClass};

fn route_class(path: Option<&str>) -> RouteClass {
    let Some(path) = path else {
        return RouteClass::Unmatched;
    };
    let path = path.strip_prefix("/api/").unwrap_or(path);
    let path = path.trim_start_matches('/');
    if path.starts_with("health/") || path == "health" {
        RouteClass::Health
    } else if path.starts_with("auth/") {
        RouteClass::Identity
    } else if path.contains("webhook") || path.starts_with("hooks/") {
        RouteClass::Webhook
    } else if path.contains("billing") || path.contains("entitlement") {
        RouteClass::Billing
    } else if path.starts_with("account") || path.starts_with("users/") {
        RouteClass::Account
    } else if path.contains("document") || path.contains("files") {
        RouteClass::Documents
    } else if path.contains("import") || path.contains("acquisition") {
        RouteClass::Import
    } else if path.contains("assistant") {
        RouteClass::Assistant
    } else if path.starts_with("projects") || path.starts_with("workspaces") {
        RouteClass::Research
    } else {
        RouteClass::Other
    }
}

fn method_class(method: &Method) -> HttpMethod {
    match method.as_str() {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        "HEAD" => HttpMethod::Head,
        "OPTIONS" => HttpMethod::Options,
        _ => HttpMethod::Other,
    }
}

pub(crate) async fn observe(
    State(metrics): State<HttpMetrics>,
    request: Request,
    next: Next,
) -> Response {
    let route = route_class(
        request
            .extensions()
            .get::<MatchedPath>()
            .map(MatchedPath::as_str),
    );
    let method = method_class(request.method());
    let started = Instant::now();
    let response = next.run(request).await;
    let sse = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"));
    metrics.response(
        route,
        method,
        response.status().as_u16(),
        started.elapsed(),
        sse,
    );
    response
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::StatusCode, middleware, routing::get};
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::{
        InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        data::{AggregatedMetrics, MetricData},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn records_rejections_but_excludes_actual_sse_from_latency_and_identifiers_from_labels() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let metrics = HttpMetrics::new(&provider.meter("http-test"));
        let router = Router::new()
            .route("/projects/{id}/reports", get(|| async { "ok" }))
            .route(
                "/assistant/events",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
                        "data: ok\n\n",
                    )
                }),
            )
            .route("/auth/login", get(|| async { StatusCode::UNAUTHORIZED }))
            .layer(middleware::from_fn_with_state(metrics, observe));
        for path in [
            "/projects/private-tenant-one/reports",
            "/projects/private-tenant-two/reports",
            "/assistant/events",
            "/auth/login",
            "/secret-unknown-path",
        ] {
            let response = router
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert!(response.status().is_success() || response.status().is_client_error());
        }
        provider.force_flush().unwrap();
        let finished = exporter.get_finished_metrics().unwrap();
        let mut response_count = 0;
        let mut latency_count = 0;
        let mut research_points = 0;
        let mut saw_rejection = false;
        for metric in finished
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
        {
            match metric.data() {
                AggregatedMetrics::U64(MetricData::Sum(sum))
                    if metric.name() == "deepref.http.requests" =>
                {
                    for point in sum.data_points() {
                        response_count += point.value();
                        let labels = format!("{:?}", point.attributes().collect::<Vec<_>>());
                        assert!(!labels.contains("private-tenant"));
                        assert!(!labels.contains("secret-unknown"));
                        assert_eq!(point.attributes().count(), 3);
                        if labels.contains("research") {
                            research_points += 1;
                            assert_eq!(point.value(), 2);
                        }
                        if labels.contains("4xx") {
                            saw_rejection = true;
                        }
                    }
                }
                AggregatedMetrics::F64(MetricData::Histogram(histogram))
                    if metric.name() == "deepref.http.response_header.duration" =>
                {
                    latency_count += histogram
                        .data_points()
                        .map(|point| point.count())
                        .sum::<u64>();
                }
                _ => {}
            }
        }
        assert_eq!(response_count, 5);
        assert_eq!(
            latency_count, 4,
            "actual SSE handshakes must not enter request latency"
        );
        assert_eq!(
            research_points, 1,
            "different resource IDs share one bounded series"
        );
        assert!(saw_rejection);
        provider.shutdown().unwrap();
    }
}
