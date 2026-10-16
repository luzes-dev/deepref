use axum::http::HeaderMap;
use deepref_domain::{Actor as DomainActor, ActorKind};

use crate::error::ApiError;

const ACTOR_KIND_HEADER: &str = "x-actor-kind";
const ACTOR_ID_HEADER: &str = "x-actor-id";

pub(crate) type Actor = DomainActor;

/// The actor a request acts as. HTTP clients are always `user`: automation and
/// system actors are built by server code, so a client claiming either would
/// forge AI provenance in the audit trail.
pub(crate) fn extract_actor(headers: &HeaderMap) -> Result<Actor, ApiError> {
    let kind = headers
        .get(ACTOR_KIND_HEADER)
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| ApiError::BadRequest("x-actor-kind must be valid ASCII".to_owned()))
        })
        .transpose()?
        .unwrap_or_else(|| "user".to_owned());
    let kind = client_actor_kind(&kind, "x-actor-kind")?;
    let id = headers
        .get(ACTOR_ID_HEADER)
        .map(|value| {
            value
                .to_str()
                .map(str::trim)
                .map(str::to_owned)
                .map_err(|_| ApiError::BadRequest("x-actor-id must be valid ASCII".to_owned()))
        })
        .transpose()?
        .unwrap_or_else(|| "local-user".to_owned());
    DomainActor::new(kind, id).map_err(|error| ApiError::BadRequest(error.to_string()))
}

/// Parses an actor kind sent by an HTTP client, accepting only `user`.
pub(crate) fn client_actor_kind(value: &str, field: &str) -> Result<ActorKind, ApiError> {
    match ActorKind::parse(value) {
        Some(ActorKind::User) => Ok(ActorKind::User),
        Some(ActorKind::Automation | ActorKind::System) => Err(ApiError::BadRequest(format!(
            "{field} must be user: automation and system actors are not accepted over HTTP"
        ))),
        None => Err(ApiError::BadRequest(format!("{field} must be user"))),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(kind: Option<&'static str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(kind) = kind {
            headers.insert(ACTOR_KIND_HEADER, HeaderValue::from_static(kind));
        }
        headers
    }

    #[test]
    fn http_clients_act_only_as_users() {
        for kind in ["automation", "system"] {
            assert!(
                matches!(
                    extract_actor(&headers(Some(kind))),
                    Err(ApiError::BadRequest(_))
                ),
                "x-actor-kind: {kind} must be rejected"
            );
        }
    }

    #[test]
    fn user_kind_is_the_default_and_the_only_accepted_kind() {
        for kind in [None, Some("user")] {
            let actor = extract_actor(&headers(kind)).map_err(|error| error.to_string());
            assert!(
                matches!(&actor, Ok(actor) if actor.kind() == ActorKind::User && actor.id() == "local-user"),
                "{kind:?}: {actor:?}"
            );
        }
    }
}
