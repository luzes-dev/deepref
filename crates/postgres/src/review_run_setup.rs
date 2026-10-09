use deepref_ai::{ResolvedModel, hash_json, provider_endpoint};
use deepref_application::BuiltInAutomationRecipe;
use deepref_domain::ProjectId;
use deepref_review::{
    ReviewDefinitionKey, ReviewSubject,
    worker::{ReviewHash, ReviewModelIdentity, ReviewRuntimeIdentity},
};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::review_runs::PostgresReviewError;

pub(crate) async fn ensure_review_automation_definition(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    recipe: BuiltInAutomationRecipe,
    actor: &deepref_domain::Actor,
) -> Result<Uuid, PostgresReviewError> {
    let row = sqlx::query(
        "SELECT id FROM configure_automation_definition($1,$2,'manual',$3,$4,'active',$5,$6)",
    )
    .bind(project_id.as_uuid())
    .bind(format!("Compiled review · {}", recipe.id()))
    .bind(recipe.id())
    .bind(recipe.version())
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .fetch_one(&mut **transaction)
    .await?;
    Ok(row.get("id"))
}

pub(crate) const fn recipe_for(key: ReviewDefinitionKey) -> BuiltInAutomationRecipe {
    match key {
        ReviewDefinitionKey::Screening => BuiltInAutomationRecipe::ReviewScreeningV1,
        ReviewDefinitionKey::DuplicateDetection => {
            BuiltInAutomationRecipe::ReviewDuplicateDetectionV1
        }
        ReviewDefinitionKey::StudyClassification => {
            BuiltInAutomationRecipe::ReviewStudyClassificationV1
        }
        ReviewDefinitionKey::StudyGrouping => BuiltInAutomationRecipe::ReviewStudyGroupingV1,
        ReviewDefinitionKey::AppraisalPrefill => BuiltInAutomationRecipe::ReviewAppraisalPrefillV1,
        ReviewDefinitionKey::DataExtraction => BuiltInAutomationRecipe::ReviewDataExtractionV1,
    }
}

pub(crate) fn protocol_version_id(
    subject: &ReviewSubject,
) -> Option<deepref_domain::ProtocolVersionId> {
    match subject {
        ReviewSubject::Screening {
            protocol_version_id,
            ..
        } => Some(*protocol_version_id),
        _ => None,
    }
}

/// The identity of one resolved route. The endpoint is the one this process calls for the
/// route's provider, so a manifest records the endpoint of the process that scheduled it.
pub(crate) fn model_identity(
    route: ResolvedModel,
) -> Result<ReviewModelIdentity, PostgresReviewError> {
    let endpoint = provider_endpoint(&route.provider)?;
    Ok(ReviewModelIdentity {
        profile: route.profile,
        provider: route.provider,
        model: route.model,
        model_version: route.model_version,
        parameters_hash: ReviewHash::parse(hash_json(&serde_json::to_value(route.parameters)?)?)?,
        endpoint,
    })
}

/// The provenance of the running build, recorded in every manifest for audit.
///
/// None of these values is a semantic identity input. The source-tree hash is
/// `DEEPREF_SOURCE_TREE_SHA`. The deployment id comes from `DEEPREF_BUILD_SHA`,
/// which the container build sets before it compiles, so `option_env!` sees it.
/// Cargo rebuilds this crate when that value changes.
pub(crate) fn runtime_identity() -> Result<ReviewRuntimeIdentity, deepref_review::ReviewError> {
    Ok(ReviewRuntimeIdentity {
        build_sha: ReviewHash::parse(env!("DEEPREF_SOURCE_TREE_SHA"))?,
        rust_version: option_env!("RUSTC_VERSION")
            .unwrap_or("workspace-toolchain")
            .to_owned(),
        target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        deployment_build_id: option_env!("DEEPREF_BUILD_SHA")
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use deepref_ai::{ModelParameters, ModelProfile, ProviderEndpoint, register_provider_endpoint};

    use super::*;

    fn route(provider: &str) -> ResolvedModel {
        ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: provider.to_owned(),
            model: "glm-test".to_owned(),
            model_version: "glm-test".to_owned(),
            parameters: ModelParameters::default(),
            route_id: None,
        }
    }

    #[test]
    fn a_route_records_the_endpoint_this_process_calls_for_its_provider() {
        let unconfigured = model_identity(route("review-setup-unconfigured")).unwrap();
        assert_eq!(unconfigured.endpoint, None);

        let endpoint =
            ProviderEndpoint::from_configured_url("https://proxy.example/zen/go/v1/?key=q")
                .unwrap();
        register_provider_endpoint("review-setup-configured", endpoint.clone()).unwrap();
        let identity = model_identity(route("review-setup-configured")).unwrap();
        assert_eq!(identity.endpoint, Some(endpoint));
        assert_eq!(identity.provider, "review-setup-configured");
    }
}
