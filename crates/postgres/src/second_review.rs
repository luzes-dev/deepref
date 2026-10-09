//! Automatic AI second review for records that are waiting for a person.
//!
//! When a stage is set to an AI second reviewer, the reviewer should screen the
//! records a person has not decided yet, without being asked. Automation-triggered
//! runs are admitted only against a passing, expert-adjudicated calibration bundle
//! for that screening stage, whose identity matches the compiled review (ADR 0004).
//! Title/abstract and full text are calibrated separately: a bundle for one stage
//! never admits the other. That gate is kept on purpose: this module schedules
//! only the work the gate admits, and says plainly when it cannot run.
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
    CalibrationRefusal, PostgresReviewError, PostgresReviewScheduler, ReviewPreparationError,
    get_ai_budget, get_ai_screening_target, get_published_protocol, resolve_autonomy_level,
    review_calibration::stage_key,
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

/// The gate kind written for a stale calibration; the other refusal kinds map to
/// `NeedsCalibration`. Matches `CalibrationRefusal::Stale` and the gate's CHECK.
const GATE_STALE: &str = "calibration_stale";

/// Counts from one sweep across every calibrated project.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SecondReviewSweep {
    /// Review runs scheduled for records that were waiting for a person.
    pub scheduled: u64,
    /// Attempts released because the calibration gate or the AI route refused.
    pub released: u64,
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

/// The newest passing calibration bundle for one screening stage, if any. Legacy
/// bundles (identity scheme 1) carry no stage and are never evidence for a stage.
/// Admission still checks that the bundle matches the compiled review.
async fn passing_calibration(
    pool: &PgPool,
    project_id: Uuid,
    stage: ScreeningStage,
) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM review_calibration_bundles
         WHERE project_id=$1 AND definition_key=$2 AND stage=$3
           AND identity_scheme>=2 AND status='passing'
         ORDER BY evaluated_at DESC, id DESC
         LIMIT 1",
    )
    .bind(project_id)
    .bind(ReviewDefinitionKey::Screening.as_str())
    .bind(stage_key(stage))
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
    if passing_calibration(pool, project_id, stage)
        .await?
        .is_none()
    {
        return Ok(SecondReviewStatus::NeedsCalibration);
    }
    match gate_refusal(pool, project_id, stage).await? {
        None => Ok(SecondReviewStatus::Automatic),
        Some(kind) if kind == GATE_STALE => Ok(SecondReviewStatus::CalibrationStale),
        // A stage or identity mismatch means no passing bundle admits this stage as
        // it stands, so it needs calibrating again, as it does when none exists.
        Some(_) => Ok(SecondReviewStatus::NeedsCalibration),
    }
}

/// The refusal kind the sweep last met for this stage, if it is still in force.
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

/// Keeps the gate record in step with the last sweep. A refusal is written with
/// its kind and, for a stale calibration, the names of the changed components.
/// A stage that schedules again, or has no bundle to refuse, is cleared.
async fn record_gate(
    pool: &PgPool,
    project_id: Uuid,
    stage: ScreeningStage,
    refusal: Option<&CalibrationRefusal>,
) -> anyhow::Result<()> {
    match refusal {
        Some(refusal) => {
            let reasons = match refusal {
                CalibrationRefusal::Stale { components } => components
                    .iter()
                    .map(|component| component.as_str().to_owned())
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            sqlx::query(
                "INSERT INTO second_review_gate (project_id, stage, refusal, reasons)
                 VALUES ($1,$2,$3,$4)
                 ON CONFLICT (project_id, stage)
                 DO UPDATE SET refusal=EXCLUDED.refusal, reasons=EXCLUDED.reasons,
                               observed_at=now()",
            )
            .bind(project_id)
            .bind(stage_key(stage))
            .bind(refusal.code())
            .bind(reasons)
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

/// One bounded pass over every project that has a passing calibration for a
/// screening stage. Projects without one cost a single query and are never sent
/// any work.
pub async fn sweep_second_reviews(pool: &PgPool) -> anyhow::Result<SecondReviewSweep> {
    let projects = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT project_id FROM review_calibration_bundles
         WHERE definition_key=$1 AND status='passing'
           AND identity_scheme>=2 AND stage IS NOT NULL
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
        // A stage without its own passing calibration has no work. Each stage is
        // gated on its own calibration, so a refusal for one never stops the other.
        let Some(bundle) = passing_calibration(pool, project_id, stage).await? else {
            continue;
        };
        let protocol = get_published_protocol(pool, project_id).await?;
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
                    // The gate or the AI route refuses every record of this stage the
                    // same way, so release this attempt and move on to the next stage.
                    // The next tick retries once the stage is calibrated or the route
                    // is configured.
                    release_attempt(pool, project_id, report_id, stage, protocol.id).await?;
                    match calibration_refusal(&error) {
                        Some(
                            refusal @ (CalibrationRefusal::Stale { .. }
                            | CalibrationRefusal::StageMismatch { .. }
                            | CalibrationRefusal::IncompatibleIdentityScheme { .. }),
                        ) => record_gate(pool, project_id, stage, Some(refusal)).await?,
                        Some(CalibrationRefusal::Missing | CalibrationRefusal::Failed) => {
                            record_gate(pool, project_id, stage, None).await?;
                        }
                        None => {}
                    }
                    sweep.released += 1;
                    tracing::info!(%project_id, ?stage, reason = %error, "automatic second review is paused for this stage");
                    break;
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

/// Refusals that apply to every record of a stage: the calibration gate, or no AI route.
fn is_environmental(error: &ReviewPreparationError) -> bool {
    if calibration_refusal(error).is_some() {
        return true;
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains("no enabled route") || message.contains("turned off")
}

/// The calibration refusal behind a scheduling error, if the error is one.
fn calibration_refusal(error: &ReviewPreparationError) -> Option<&CalibrationRefusal> {
    match error {
        ReviewPreparationError::Review(PostgresReviewError::CalibrationRefused(refusal)) => {
            Some(refusal)
        }
        _ => None,
    }
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
