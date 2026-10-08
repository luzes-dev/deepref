//! Automatic AI second review for records that are waiting for a person.
//!
//! When a stage is set to an AI second reviewer, the reviewer should screen the
//! records a person has not decided yet, without being asked. Automation-triggered
//! runs are admitted only against a passing, expert-adjudicated calibration bundle
//! whose semantic hash matches the compiled review (ADR 0004). That gate is kept
//! on purpose: this module schedules only the work the gate admits, and says
//! plainly when it cannot run.
//!
//! The sweep is bounded. It schedules at most [`SECOND_REVIEW_BATCH`] records per
//! project per tick, makes one automatic attempt per record, stage and protocol
//! version, and does nothing once the project's AI budget is spent.

use deepref_application::workflows::{AutonomyLevel, AutonomyTask};
use deepref_domain::{Actor, ActorKind, ProjectId, ProtocolVersionId, ReportId, ScreeningStage};
use deepref_review::{
    CalibrationBundleId, ReviewDefinitionKey, ReviewOrigin, ReviewScheduler, ReviewSubject,
    ScheduleReviewRun,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    PostgresReviewError, PostgresReviewScheduler, ReviewPreparationError, get_ai_budget,
    get_ai_screening_target, get_published_protocol, resolve_autonomy_level,
};

/// Most records one project may send to the reviewer in one sweep.
pub const SECOND_REVIEW_BATCH: i64 = 5;

/// What the second-reviewer setting does for one screening stage right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondReviewStatus {
    /// The stage is not set to an AI second reviewer.
    NotEnabled,
    /// The stage is an AI second reviewer and a passing calibration admits it.
    Automatic,
    /// The stage is an AI second reviewer, but no passing calibration exists yet.
    NeedsCalibration,
    /// The stage is an AI second reviewer, but the passing calibration was made for an
    /// earlier compiled review (another protocol, prompt or model), so it is refused.
    CalibrationStale,
}

impl SecondReviewStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotEnabled => "not_enabled",
            Self::Automatic => "automatic",
            Self::NeedsCalibration => "needs_calibration",
            Self::CalibrationStale => "calibration_stale",
        }
    }
}

/// Counts from one sweep across every calibrated project.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SecondReviewSweep {
    /// Review runs scheduled for records that were waiting for a person.
    pub scheduled: u64,
    /// Attempts released because the calibration gate or the AI route refused.
    pub released: u64,
}

fn stage_key(stage: ScreeningStage) -> &'static str {
    match stage {
        ScreeningStage::TitleAbstract => "title_abstract",
        ScreeningStage::FullText => "full_text",
    }
}

fn stage_of(task: AutonomyTask) -> Option<ScreeningStage> {
    match task {
        AutonomyTask::TitleAbstractScreening => Some(ScreeningStage::TitleAbstract),
        AutonomyTask::FullTextScreening => Some(ScreeningStage::FullText),
        _ => None,
    }
}

async fn autonomy_level(
    pool: &PgPool,
    project_id: Uuid,
    task: AutonomyTask,
) -> anyhow::Result<AutonomyLevel> {
    resolve_autonomy_level(pool, project_id, task)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

/// The newest passing calibration bundle for the screening definition, if any.
/// Admission still checks that its semantic hash matches the compiled review.
async fn passing_calibration(pool: &PgPool, project_id: Uuid) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM review_calibration_bundles
         WHERE project_id=$1 AND definition_key=$2 AND status='passing'
         ORDER BY evaluated_at DESC, id DESC
         LIMIT 1",
    )
    .bind(project_id)
    .bind(ReviewDefinitionKey::Screening.as_str())
    .fetch_optional(pool)
    .await?)
}

/// Says whether automatic second review runs for one stage, and why not if it does not.
pub async fn second_review_status(
    pool: &PgPool,
    project_id: Uuid,
    task: AutonomyTask,
) -> anyhow::Result<SecondReviewStatus> {
    let Some(stage) = stage_of(task) else {
        return Ok(SecondReviewStatus::NotEnabled);
    };
    if autonomy_level(pool, project_id, task).await? != AutonomyLevel::SecondReviewer {
        return Ok(SecondReviewStatus::NotEnabled);
    }
    if passing_calibration(pool, project_id).await?.is_none() {
        return Ok(SecondReviewStatus::NeedsCalibration);
    }
    if gate_refusal(pool, project_id, stage).await?.is_some() {
        return Ok(SecondReviewStatus::CalibrationStale);
    }
    Ok(SecondReviewStatus::Automatic)
}

/// The refusal the sweep last met for this stage, if it is still in force.
async fn gate_refusal(
    pool: &PgPool,
    project_id: Uuid,
    stage: ScreeningStage,
) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT refusal FROM second_review_gate WHERE project_id=$1 AND stage=$2",
    )
    .bind(project_id)
    .bind(stage_key(stage))
    .fetch_optional(pool)
    .await?)
}

/// Keeps the gate record in step with the last sweep: a stale calibration is written,
/// and a stage that schedules again (or has no bundle to refuse) is cleared.
async fn record_gate(
    pool: &PgPool,
    project_id: Uuid,
    stage: ScreeningStage,
    refusal: Option<&str>,
) -> anyhow::Result<()> {
    match refusal {
        Some(refusal) => {
            sqlx::query(
                "INSERT INTO second_review_gate (project_id, stage, refusal)
                 VALUES ($1,$2,$3)
                 ON CONFLICT (project_id, stage)
                 DO UPDATE SET refusal=EXCLUDED.refusal, observed_at=now()",
            )
            .bind(project_id)
            .bind(stage_key(stage))
            .bind(refusal)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM second_review_gate WHERE project_id=$1 AND stage=$2")
                .bind(project_id)
                .bind(stage_key(stage))
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// One bounded pass over every project that has a passing screening calibration.
/// Projects without one cost a single query and are never sent any work.
pub async fn sweep_second_reviews(pool: &PgPool) -> anyhow::Result<SecondReviewSweep> {
    let projects = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT project_id FROM review_calibration_bundles
         WHERE definition_key=$1 AND status='passing'
         ORDER BY project_id",
    )
    .bind(ReviewDefinitionKey::Screening.as_str())
    .fetch_all(pool)
    .await?;
    let mut total = SecondReviewSweep::default();
    for project_id in projects {
        match sweep_project(pool, project_id).await {
            Ok(sweep) => {
                total.scheduled += sweep.scheduled;
                total.released += sweep.released;
            }
            Err(error) => {
                tracing::warn!(%error, %project_id, "second review sweep skipped a project");
            }
        }
    }
    Ok(total)
}

async fn sweep_project(pool: &PgPool, project_id: Uuid) -> anyhow::Result<SecondReviewSweep> {
    let mut sweep = SecondReviewSweep::default();
    let budget = get_ai_budget(pool, project_id).await?;
    if budget.spent_micros >= budget.budget_micros {
        return Ok(sweep);
    }
    let Some(bundle) = passing_calibration(pool, project_id).await? else {
        return Ok(sweep);
    };
    let protocol = get_published_protocol(pool, project_id).await?;
    for task in [
        AutonomyTask::TitleAbstractScreening,
        AutonomyTask::FullTextScreening,
    ] {
        let Some(stage) = stage_of(task) else {
            continue;
        };
        if autonomy_level(pool, project_id, task).await? != AutonomyLevel::SecondReviewer {
            continue;
        }
        let remaining = SECOND_REVIEW_BATCH - sweep.scheduled as i64;
        if remaining <= 0 {
            break;
        }
        let reports = waiting_reports(pool, project_id, stage, protocol.id, remaining).await?;
        for report_id in reports {
            let claim = sqlx::query(
                "INSERT INTO second_review_attempts (project_id, report_id, stage, protocol_version_id)
                 VALUES ($1,$2,$3,$4)
                 ON CONFLICT DO NOTHING",
            )
            .bind(project_id)
            .bind(report_id)
            .bind(stage_key(stage))
            .bind(protocol.id)
            .execute(pool)
            .await?;
            if claim.rows_affected() == 0 {
                continue;
            }
            let target = get_ai_screening_target(pool, project_id, report_id).await?;
            let command = ScheduleReviewRun {
                project_id: ProjectId::new(project_id),
                definition: ReviewDefinitionKey::Screening,
                subject: ReviewSubject::Screening {
                    report_id: ReportId::new(report_id),
                    stage,
                    protocol_version_id: ProtocolVersionId::new(protocol.id),
                    expected_revision: target.expected_revision,
                },
                origin: ReviewOrigin::AutomationTriggered {
                    calibration_bundle_id: CalibrationBundleId::new(bundle)?,
                },
                actor: Actor::new(ActorKind::Automation, "second-reviewer")?,
            };
            match PostgresReviewScheduler::new(pool).schedule(command).await {
                Ok(_) => {
                    record_gate(pool, project_id, stage, None).await?;
                    sweep.scheduled += 1;
                }
                Err(error) if is_environmental(&error) => {
                    // The gate or the AI route refuses every record the same way, so
                    // release this attempt and stop. The next tick retries once the
                    // project is calibrated or the route is configured.
                    release_attempt(pool, project_id, report_id, stage, protocol.id).await?;
                    if is_stale(&error) {
                        record_gate(pool, project_id, stage, Some("calibration_stale")).await?;
                    } else if is_calibration_refusal(&error) {
                        record_gate(pool, project_id, stage, None).await?;
                    }
                    sweep.released += 1;
                    tracing::info!(%project_id, reason = %error, "automatic second review is paused");
                    return Ok(sweep);
                }
                Err(error) => {
                    // A record-specific refusal keeps its attempt, so it cannot block
                    // the rest of the queue or be retried on every tick.
                    tracing::warn!(%error, %project_id, %report_id, "second review could not be scheduled");
                }
            }
            if sweep.scheduled as i64 >= SECOND_REVIEW_BATCH {
                return Ok(sweep);
            }
        }
    }
    Ok(sweep)
}

/// Records that are waiting for a person at one stage, with no automatic attempt yet
/// for this protocol version and no AI reviewer decision already on file.
async fn waiting_reports(
    pool: &PgPool,
    project_id: Uuid,
    stage: ScreeningStage,
    protocol_version_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT pr.report_id
         FROM project_reports pr
         LEFT JOIN screening_state s ON s.project_id=pr.project_id AND s.report_id=pr.report_id
         WHERE pr.project_id=$1
           AND (
             ($2='title_abstract' AND COALESCE(s.title_abstract_status,'unscreened')='unscreened')
             OR ($2='full_text' AND s.title_abstract_status='include' AND s.full_text_status='unscreened'
                 AND EXISTS (SELECT 1 FROM documents d
                             WHERE d.project_id=pr.project_id AND d.report_id=pr.report_id
                               AND d.status='available'))
           )
           AND NOT EXISTS (
             SELECT 1 FROM second_review_attempts a
             WHERE a.project_id=pr.project_id AND a.report_id=pr.report_id
               AND a.stage=$2 AND a.protocol_version_id=$4)
           AND NOT EXISTS (
             SELECT 1 FROM ai_reviewer_decisions r
             WHERE r.project_id=pr.project_id AND r.report_id=pr.report_id
               AND r.stage=$2 AND r.voided_at IS NULL)
         ORDER BY pr.created_at, pr.report_id
         LIMIT $3",
    )
    .bind(project_id)
    .bind(stage_key(stage))
    .bind(limit)
    .bind(protocol_version_id)
    .fetch_all(pool)
    .await?)
}

async fn release_attempt(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    stage: ScreeningStage,
    protocol_version_id: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        "DELETE FROM second_review_attempts
         WHERE project_id=$1 AND report_id=$2 AND stage=$3 AND protocol_version_id=$4",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(stage_key(stage))
    .bind(protocol_version_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Refusals that apply to every record: the calibration gate, or no AI route.
fn is_environmental(error: &ReviewPreparationError) -> bool {
    if is_calibration_refusal(error) {
        return true;
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains("no enabled route") || message.contains("turned off")
}

fn is_calibration_refusal(error: &ReviewPreparationError) -> bool {
    matches!(
        error,
        ReviewPreparationError::Review(
            PostgresReviewError::CalibrationMissing
                | PostgresReviewError::CalibrationFailed
                | PostgresReviewError::CalibrationStale
        )
    )
}

/// The calibration exists and passes, but it was made for another compiled review.
fn is_stale(error: &ReviewPreparationError) -> bool {
    matches!(
        error,
        ReviewPreparationError::Review(PostgresReviewError::CalibrationStale)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names_are_stable_api_values() {
        assert_eq!(SecondReviewStatus::NotEnabled.as_str(), "not_enabled");
        assert_eq!(SecondReviewStatus::Automatic.as_str(), "automatic");
        assert_eq!(
            SecondReviewStatus::NeedsCalibration.as_str(),
            "needs_calibration"
        );
    }

    #[test]
    fn only_screening_stages_have_an_automatic_second_review() {
        assert_eq!(
            stage_of(AutonomyTask::TitleAbstractScreening),
            Some(ScreeningStage::TitleAbstract)
        );
        assert_eq!(
            stage_of(AutonomyTask::FullTextScreening),
            Some(ScreeningStage::FullText)
        );
        assert_eq!(stage_of(AutonomyTask::Extraction), None);
    }
}
