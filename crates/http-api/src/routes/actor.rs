use axum::http::HeaderMap;
use deepref_domain::{Actor as DomainActor, ActorKind};

use crate::error::ApiError;

const ACTOR_KIND_HEADER: &str = "x-actor-kind";
const ACTOR_ID_HEADER: &str = "x-actor-id";

pub(crate) type Actor = DomainActor;

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
    let kind = ActorKind::parse(&kind).ok_or_else(|| {
        ApiError::BadRequest("x-actor-kind must be user, automation, or system".to_owned())
    })?;
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
