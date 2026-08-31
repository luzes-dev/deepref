use deepref_ai::{ResolvedModel, hash_json};
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

pub(crate) fn model_identity(
    route: ResolvedModel,
) -> Result<ReviewModelIdentity, PostgresReviewError> {
    Ok(ReviewModelIdentity {
        profile: route.profile,
        provider: route.provider,
        model: route.model,
        model_version: route.model_version,
        parameters_hash: ReviewHash::parse(hash_json(&serde_json::to_value(route.parameters)?)?)?,
    })
}

pub(crate) fn runtime_identity() -> ReviewRuntimeIdentity {
    ReviewRuntimeIdentity {
        build_sha: ReviewHash::parse(env!("DEEPREF_SEMANTIC_BUILD_SHA"))
            .expect("build script emits a SHA-256 runtime identity"),
        rust_version: option_env!("RUSTC_VERSION")
            .unwrap_or("workspace-toolchain")
            .to_owned(),
        target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
    }
}
