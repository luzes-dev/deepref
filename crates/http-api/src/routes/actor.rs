use axum::{extract::FromRequestParts, http::request::Parts};
use deepref_domain::{Actor as DomainActor, ActorKind, Principal};

use crate::{
    auth::AuthenticatedPrincipal,
    error::{ApiError, AuthFailure},
};

pub(crate) type Actor = DomainActor;

/// The human actor derived from the authenticated request principal.
///
/// The browser's legacy `x-actor-id` and `x-actor-kind` headers are deliberately
/// not read. During a rolling deploy an N-1 browser may still send them; the
/// authenticated principal remains the only source of attribution.
#[derive(Debug, Clone)]
pub(crate) struct AuthenticatedActor {
    pub(crate) actor: Actor,
    pub(crate) user_id: deepref_domain::UserId,
}

impl<S: Send + Sync> FromRequestParts<S> for AuthenticatedActor {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let AuthenticatedPrincipal(principal) =
            AuthenticatedPrincipal::from_request_parts(parts, state).await?;
        let user_id = principal.user_id().ok_or_else(|| {
            ApiError::Forbidden("scientific HTTP writes require a human user principal".to_owned())
        })?;
        let actor = actor_from_principal(principal)?;
        Ok(Self { actor, user_id })
    }
}

/// Convert a browser principal to the only human actor representation stored
/// by new scientific writes: the canonical `UserId` UUID string.
pub(crate) fn actor_from_principal(principal: Principal) -> Result<Actor, ApiError> {
    let Principal::User(user) = principal else {
        return Err(ApiError::Auth(AuthFailure::Unauthenticated));
    };
    DomainActor::new(ActorKind::User, user.user_id.as_uuid().to_string())
        .map_err(|error| ApiError::Internal(error.into()))
}

#[cfg(test)]
mod tests {
    use deepref_domain::{Principal, UserId};
    use uuid::Uuid;

    use super::*;

    #[test]
    fn actor_uses_the_authenticated_user_id() {
        let user_id = UserId::new(Uuid::from_u128(0x1234));
        let actor = actor_from_principal(Principal::user(user_id)).expect("user principal");
        assert_eq!(actor.kind(), ActorKind::User);
        assert_eq!(actor.id(), "00000000-0000-0000-0000-000000001234");
    }

    #[test]
    fn system_and_service_principals_cannot_be_http_actors() {
        for principal in [Principal::system(), Principal::service()] {
            assert!(actor_from_principal(principal).is_err());
        }
    }
}
