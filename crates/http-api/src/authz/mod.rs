//! The central Policy Enforcement Point (handoff 03b).
//!
//! One middleware, layered on the router after the session layer, decides
//! every request from the static table in [`policy`]. Handlers never check
//! roles. A route with no row is denied and logged: the table is the coverage
//! matrix, and adding a route without a row fails closed here and fails the
//! `authz-matrix` check in CI.
//!
//! A project that does not exist, a project in another workspace and a
//! principal without the permission are one outcome, rendered exactly like the
//! `404` a nonexistent project already produced.

pub mod matrix;
pub mod policy;

use axum::{
    extract::{RawPathParams, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use deepref_app::authorization::{Denial, DeniedFor, DeniedResource};
use deepref_application::authorization::AuthzError;
use deepref_domain::{
    ProjectId,
    authorization::{Permission, Resource},
};
use uuid::Uuid;

use crate::{
    auth::SessionContext,
    error::{ApiError, AuthFailure},
    state::AppState,
};
use policy::{ResourceFrom, Rule};

/// What the PEP does with the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthzMode {
    /// Every non-open request is decided by the policy.
    Enforced,
    /// Authentication is off in a development environment: there is no
    /// principal to authorize, so requests pass. Local development only.
    DevelopmentBypass,
    /// Authentication is off outside development. Authorization cannot be
    /// bypassed in production, so every non-open request is refused.
    Refused,
}

impl AuthzMode {
    pub const fn for_runtime(auth_enabled: bool, development: bool) -> Self {
        if auth_enabled {
            Self::Enforced
        } else if development {
            Self::DevelopmentBypass
        } else {
            Self::Refused
        }
    }
}

#[derive(Clone)]
pub(crate) struct Pep {
    pub(crate) mode: AuthzMode,
    pub(crate) state: AppState,
}

/// The denial every "no" becomes: byte-identical to the `404` for a project
/// that does not exist.
pub(crate) fn denied_response() -> Response {
    ApiError::NotFound("resource not found".to_owned()).into_response()
}

fn method_not_allowed(allowed: &[&str]) -> Response {
    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(&allowed.join(", ")) {
        response
            .headers_mut()
            .insert(axum::http::header::ALLOW, value);
    }
    response
}

/// What a denial needs from the request, copied out so no borrow of the
/// request body crosses an await.
struct Caller {
    user: Option<deepref_domain::UserId>,
    request_id: Option<String>,
}

pub(crate) async fn enforce(
    State(pep): State<Pep>,
    params: RawPathParams,
    request: Request,
    next: Next,
) -> Response {
    let matched = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|path| path.as_str().to_owned());
    let Some(matched) = matched else {
        // No route matched: the router's own 404 answers. No handler runs.
        return next.run(request).await;
    };
    // The router answers HEAD from the GET handler, so HEAD is decided as GET.
    let method = match request.method().as_str() {
        "HEAD" => "GET".to_owned(),
        other => other.to_owned(),
    };
    let query = request.uri().query().map(str::to_owned);
    let session = request.extensions().get::<SessionContext>().cloned();
    let principal = request
        .extensions()
        .get::<deepref_domain::Principal>()
        .copied()
        .or_else(|| session.as_ref().map(|session| session.principal));
    let caller = Caller {
        user: session
            .as_ref()
            .and_then(|session| session.principal.user_id()),
        request_id: request
            .extensions()
            .get::<Uuid>()
            .map(std::string::ToString::to_string),
    };
    let Some(row) = policy::lookup(&method, &matched) else {
        // The router can serve a method no row lists: `routes!` with several
        // paths registers every handler on every path. Such a combination is
        // answered `405` here and never reaches a handler.
        let allowed: Vec<&str> = policy::ROUTES
            .iter()
            .filter(|row| row.path == matched)
            .map(|row| row.method.as_str())
            .collect();
        if !allowed.is_empty() {
            return method_not_allowed(&allowed);
        }
        tracing::error!(
            %method, route = %matched,
            "route has no authorization policy row; denying (add it to authz/policy.rs)"
        );
        return deny(
            &pep.state,
            &caller,
            DeniedFor::RouteUnlisted,
            DeniedResource::None,
        )
        .await;
    };
    if row.rule == Rule::Open {
        return next.run(request).await;
    }
    match pep.mode {
        AuthzMode::DevelopmentBypass => return next.run(request).await,
        AuthzMode::Refused => {
            return ApiError::Configuration(
                "authentication must be enabled outside development".to_owned(),
            )
            .into_response();
        }
        AuthzMode::Enforced => {}
    }
    let Some(principal) = principal else {
        return ApiError::Auth(AuthFailure::Unauthenticated).into_response();
    };
    if matches!(principal, deepref_domain::Principal::Service(_)) && !policy::machine_safe(row) {
        pep.state
            .record_auth_rejection(
                deepref_app::auth_rejections::RejectionReason::ApiKeyRouteDenied,
                None,
            )
            .await;
        return denied_response();
    }
    let (permission, source, optional) = match row.rule {
        Rule::Require(permission, source) => (permission, source, false),
        Rule::RequireIfPresent(permission, source) => (permission, source, true),
        _ => (Permission::ProjectView, ResourceFrom::PathParam(""), false),
    };
    match row.rule {
        Rule::Open | Rule::Authenticated | Rule::Handler(_) => {
            run_authorized(&pep.state, session.as_ref(), row, request, next).await
        }
        Rule::PlatformOperator => {
            deny(
                &pep.state,
                &caller,
                DeniedFor::PlatformOperator,
                DeniedResource::None,
            )
            .await
        }
        Rule::Require(..) | Rule::RequireIfPresent(..) => {
            let raw = match source {
                ResourceFrom::PathParam(name) => params
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_owned()),
                ResourceFrom::Query(name) => query.as_deref().and_then(|query| {
                    serde_urlencoded::from_str::<Vec<(String, String)>>(query)
                        .ok()
                        .and_then(|pairs| {
                            pairs
                                .into_iter()
                                .find(|(key, _)| key == name)
                                .map(|(_, v)| v)
                        })
                }),
            };
            if optional && raw.is_none() {
                return run_authorized(&pep.state, session.as_ref(), row, request, next).await;
            }
            let id = raw.and_then(|value| Uuid::parse_str(&value).ok());
            let Some(id) = id else {
                return deny(
                    &pep.state,
                    &caller,
                    DeniedFor::Permission(permission),
                    DeniedResource::Project(None),
                )
                .await;
            };
            let project = ProjectId::new(id);
            match pep
                .state
                .authorizer
                .require(&principal, permission, Resource::Project(project))
                .await
            {
                Ok(()) => run_authorized(&pep.state, session.as_ref(), row, request, next).await,
                Err(AuthzError::Denied(_)) => {
                    if matches!(principal, deepref_domain::Principal::Service(_)) {
                        pep.state
                            .record_auth_rejection(
                                deepref_app::auth_rejections::RejectionReason::ApiKeyRouteDenied,
                                None,
                            )
                            .await;
                    }
                    deny(
                        &pep.state,
                        &caller,
                        DeniedFor::Permission(permission),
                        DeniedResource::Project(Some(project)),
                    )
                    .await
                }
                Err(AuthzError::Store(error)) => store_failure(&error),
            }
        }
    }
}

/// Only explicitly classified authenticated commands produce evidence. The
/// short receipt transaction ends before a handler acquires a connection.
async fn run_authorized(
    state: &AppState,
    session: Option<&SessionContext>,
    row: &policy::RouteRule,
    request: Request,
    next: Next,
) -> Response {
    if !policy::human_command_attempt(row) {
        return next.run(request).await;
    }
    let Some(session) = session else {
        return ApiError::Auth(AuthFailure::Unauthenticated).into_response();
    };
    let deepref_domain::Principal::User(user) = session.principal else {
        return ApiError::Auth(AuthFailure::Unauthenticated).into_response();
    };
    match deepref_app::account_activity::record_product_command_attempt(
        &state.pool,
        user.user_id,
        session.session_id,
    )
    .await
    {
        Ok(true) => next.run(request).await,
        Ok(false) => ApiError::Auth(AuthFailure::Unauthenticated).into_response(),
        Err(error) => ApiError::Internal(
            anyhow::Error::new(error).context("could not record human product activity"),
        )
        .into_response(),
    }
}

fn store_failure(error: &str) -> Response {
    tracing::error!(%error, "authorization data could not be read");
    ApiError::Internal(anyhow::anyhow!("authorization is unavailable")).into_response()
}

/// Audits the denial (bounded) and answers it.
async fn deny(
    state: &AppState,
    caller: &Caller,
    denied_for: DeniedFor,
    resource: DeniedResource,
) -> Response {
    if let Some(user) = caller.user {
        record_denial(
            state,
            user,
            caller.request_id.as_deref(),
            denied_for,
            resource,
        )
        .await;
    }
    denied_response()
}

pub(crate) async fn record_denial(
    state: &AppState,
    user: deepref_domain::UserId,
    request_id: Option<&str>,
    denied_for: DeniedFor,
    resource: DeniedResource,
) {
    let request_id = request_id
        .map(str::to_owned)
        .or_else(crate::error::correlation_id);
    let denial = Denial {
        user,
        denied_for,
        resource,
        request_id: request_id.as_deref(),
    };
    if let Err(error) = state.denials.record(denial).await {
        tracing::warn!(%error, "authorization denial could not be audited");
    }
}

/// Whether the caller holds `permission` on `resource`, for handlers that
/// shape a response by privilege rather than refuse it (credential
/// redaction). Not audited: nothing was denied. With authentication off
/// (local development) it is true.
pub(crate) async fn holds(
    state: &AppState,
    session: Option<&SessionContext>,
    permission: Permission,
    resource: Resource,
) -> Result<bool, ApiError> {
    let Some(session) = session else {
        return Ok(true);
    };
    match state
        .authorizer
        .require(&session.principal, permission, resource)
        .await
    {
        Ok(()) => Ok(true),
        Err(AuthzError::Denied(_)) => Ok(false),
        Err(AuthzError::Store(error)) => {
            tracing::error!(%error, "authorization data could not be read");
            Err(ApiError::Internal(anyhow::anyhow!(
                "authorization is unavailable"
            )))
        }
    }
}

/// The audited decision for dispatch-level checks (assistant tools, plan
/// actions): the same decision point and the same bounded, sampled denial audit
/// as the middleware. `principal` is `None` only with authentication off
/// (local development), where nothing is decided.
pub(crate) async fn authorize_audited(
    state: &AppState,
    principal: Option<deepref_domain::Principal>,
    permission: Permission,
    resource: Resource,
) -> Result<(), AuthzError> {
    let Some(principal) = principal else {
        return Ok(());
    };
    deepref_app::authorization::AuditedAuthorizer::new(
        std::sync::Arc::clone(&state.authorizer),
        std::sync::Arc::clone(&state.denials),
    )
    .require(&principal, permission, resource, None)
    .await
}

/// Handler-side check for a resource only the body names (`Rule::Handler`).
/// A denial is audited and returned as the same `404`.
pub(crate) async fn require_for_handler(
    state: &AppState,
    session: Option<&SessionContext>,
    permission: Permission,
    resource: Resource,
) -> Result<(), ApiError> {
    let Some(session) = session else {
        // Authentication is off (development bypass): nothing to authorize.
        return Ok(());
    };
    match state
        .authorizer
        .require(&session.principal, permission, resource)
        .await
    {
        Ok(()) => Ok(()),
        Err(AuthzError::Denied(_)) => {
            if let Some(user) = session.principal.user_id() {
                let resource = match resource {
                    Resource::Workspace(id) => DeniedResource::Workspace(Some(id)),
                    Resource::Project(id) => DeniedResource::Project(Some(id)),
                };
                record_denial(
                    state,
                    user,
                    None,
                    DeniedFor::Permission(permission),
                    resource,
                )
                .await;
            }
            Err(ApiError::NotFound("resource not found".to_owned()))
        }
        Err(AuthzError::Store(error)) => {
            tracing::error!(%error, "authorization data could not be read");
            Err(ApiError::Internal(anyhow::anyhow!(
                "authorization is unavailable"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        middleware::from_fn_with_state,
        routing::get,
    };
    use deepref_app::identity::UserRecord;
    use deepref_domain::{ExternalUserId, UserId, UserStatus};
    use tower::ServiceExt;

    use super::*;

    fn lazy_state() -> AppState {
        // A pool no test may query, failing an acquire immediately instead of
        // waiting out the default 30-second timeout when a denial audit write
        // reaches it.
        AppState::new(
            sqlx::postgres::PgPoolOptions::new()
                .acquire_timeout(std::time::Duration::from_millis(250))
                .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/unused")
                .unwrap(),
        )
    }

    fn session() -> SessionContext {
        let id = UserId::new(Uuid::new_v4());
        SessionContext {
            principal: deepref_domain::Principal::user(id),
            session_id: Uuid::new_v4(),
            user: UserRecord {
                id,
                external_user_id: ExternalUserId::parse("user_x").unwrap(),
                email: "x@example.com".to_owned(),
                display_name: None,
                status: UserStatus::Active,
            },
        }
    }

    /// A router with `path` served by a handler that answers 200, the PEP in
    /// `mode`, and (optionally) a session already resolved, as the session
    /// layer would have.
    fn app(path: &'static str, mode: AuthzMode, with_session: bool) -> Router {
        Router::new()
            .route(path, get(|| async { "reached" }))
            .layer(from_fn_with_state(
                Pep {
                    mode,
                    state: lazy_state(),
                },
                enforce,
            ))
            .layer(axum::middleware::from_fn(
                move |mut request: Request<Body>, next: axum::middleware::Next| async move {
                    if with_session {
                        request.extensions_mut().insert(session());
                    }
                    next.run(request).await
                },
            ))
    }

    async fn status(app: Router, path: &str) -> StatusCode {
        app.oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn a_route_with_no_policy_row_is_denied_not_served() {
        let app = app("/not/in/the/table", AuthzMode::Enforced, true);
        assert_eq!(
            status(app, "/not/in/the/table").await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn an_unlisted_route_is_denied_even_without_a_session_check() {
        let app = app("/not/in/the/table", AuthzMode::DevelopmentBypass, false);
        assert_eq!(
            status(app, "/not/in/the/table").await,
            StatusCode::NOT_FOUND,
            "the development bypass skips decisions, not the table: a route with no row is a bug"
        );
    }

    #[tokio::test]
    async fn a_method_no_row_lists_is_405_and_never_reaches_a_handler() {
        let app = Router::new()
            .route(
                "/me",
                get(|| async { "reached" }).post(|| async { "reached" }),
            )
            .layer(from_fn_with_state(
                Pep {
                    mode: AuthzMode::Enforced,
                    state: lazy_state(),
                },
                enforce,
            ))
            .layer(axum::middleware::from_fn(
                |mut request: Request<Body>, next: axum::middleware::Next| async move {
                    request.extensions_mut().insert(session());
                    next.run(request).await
                },
            ));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()[axum::http::header::ALLOW], "GET");
    }

    #[tokio::test]
    async fn open_rows_are_served_without_a_session() {
        let app = app("/health/live", AuthzMode::Enforced, false);
        assert_eq!(status(app, "/health/live").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn an_authenticated_row_needs_a_session() {
        let without = app("/me", AuthzMode::Enforced, false);
        assert_eq!(status(without, "/me").await, StatusCode::UNAUTHORIZED);
        let with = app("/me", AuthzMode::Enforced, true);
        assert_eq!(status(with, "/me").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn outside_development_nothing_but_open_rows_is_served_without_authentication() {
        let refused = app("/me", AuthzMode::Refused, false);
        assert_eq!(
            status(refused, "/me").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        let open = app("/health/live", AuthzMode::Refused, false);
        assert_eq!(status(open, "/health/live").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_development_bypass_serves_listed_routes() {
        let bypass = app("/me", AuthzMode::DevelopmentBypass, false);
        assert_eq!(status(bypass, "/me").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_platform_operator_row_denies_every_user() {
        let app = Router::new()
            .route("/settings", axum::routing::patch(|| async { "reached" }))
            .layer(from_fn_with_state(
                Pep {
                    mode: AuthzMode::Enforced,
                    state: lazy_state(),
                },
                enforce,
            ))
            .layer(axum::middleware::from_fn(
                |mut request: Request<Body>, next: axum::middleware::Next| async move {
                    request.extensions_mut().insert(session());
                    next.run(request).await
                },
            ));
        let response = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn mode_follows_authentication_and_environment() {
        assert_eq!(AuthzMode::for_runtime(true, false), AuthzMode::Enforced);
        assert_eq!(AuthzMode::for_runtime(true, true), AuthzMode::Enforced);
        assert_eq!(
            AuthzMode::for_runtime(false, true),
            AuthzMode::DevelopmentBypass
        );
        assert_eq!(AuthzMode::for_runtime(false, false), AuthzMode::Refused);
    }
}
