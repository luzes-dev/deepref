//! Opinion exposures: when an AI screening opinion became visible to a person,
//! and which human/AI pairs can still count as independent evidence.
//!
//! A second reviewer agrees or disagrees with a person only as evidence of
//! independence if the person decided before seeing the AI's verdict. Every
//! route that showed a verdict is recorded in `ai_opinion_exposures` (see
//! migration 0049). `independent_reviewer_pairs` then leaves out any pair whose
//! AI opinion was visible before the person's decision.
//!
//! Exposure is keyed by record and stage, not by AI run, on purpose. Opinions
//! from the same model on the same record are correlated, so any AI verdict a
//! person saw first contaminates a later decision, whichever run produced it.

use chrono::{DateTime, Utc};
use deepref_domain::Actor;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

/// The route through which an AI opinion became visible. The SQL values are
/// the CHECK constraint of `ai_opinion_exposures.exposure_source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposureSource {
    /// A workflow screening step attached the verdict to its records, or a
    /// second-opinion step put it in its run output.
    WorkflowRunOutput,
    /// A screening suggestion became available on a project read path.
    ScreeningSuggestion,
    /// The second reviewer's opinion was returned after the person had decided:
    /// by the reviewer-decision API, or by the activity feed.
    ReviewerOpinionReveal,
    /// Backfill: the activity feed showed the opinion before blinding existed.
    ActivityFeedLegacy,
    /// Backfill: a screening proposal was listable before blinding existed.
    ProposalLegacy,
}

impl ExposureSource {
    /// The stored value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkflowRunOutput => "workflow_run_output",
            Self::ScreeningSuggestion => "screening_suggestion",
            Self::ReviewerOpinionReveal => "reviewer_opinion_reveal",
            Self::ActivityFeedLegacy => "activity_feed_legacy",
            Self::ProposalLegacy => "proposal_legacy",
        }
    }
}

/// One exposure to record. An `exposure_possible_at` of `None` is the database
/// clock at the insert, which conservatively marks availability.
#[derive(Debug, Clone)]
pub struct NewExposure {
    pub project_id: Uuid,
    pub report_id: Uuid,
    /// `title_abstract` or `full_text`.
    pub stage: String,
    pub source: ExposureSource,
    /// The one actor who saw the opinion. `None` means anyone with project access.
    pub audience: Option<Actor>,
    pub ai_reviewer_decision_id: Option<Uuid>,
    pub proposal_id: Option<Uuid>,
    pub ai_run_id: Option<Uuid>,
    pub workflow_run_id: Option<Uuid>,
    pub exposure_possible_at: Option<DateTime<Utc>>,
}

impl NewExposure {
    /// An exposure visible to everyone in the project, at the current database time.
    pub fn new(
        project_id: Uuid,
        report_id: Uuid,
        stage: impl Into<String>,
        source: ExposureSource,
    ) -> Self {
        Self {
            project_id,
            report_id,
            stage: stage.into(),
            source,
            audience: None,
            ai_reviewer_decision_id: None,
            proposal_id: None,
            ai_run_id: None,
            workflow_run_id: None,
            exposure_possible_at: None,
        }
    }
}

const INSERT_EXPOSURES: &str = "
    INSERT INTO ai_opinion_exposures
      (project_id, report_id, stage, exposure_source, audience_actor_kind, audience_actor_id,
       ai_reviewer_decision_id, proposal_id, ai_run_id, workflow_run_id, exposure_possible_at)
    SELECT x.project_id, x.report_id, x.stage, x.source, x.audience_kind, x.audience_id,
           x.decision_id, x.proposal_id, x.ai_run_id, x.workflow_run_id,
           COALESCE(x.possible_at, now())
    FROM UNNEST($1::uuid[], $2::uuid[], $3::text[], $4::text[], $5::text[], $6::text[],
                $7::uuid[], $8::uuid[], $9::uuid[], $10::uuid[], $11::timestamptz[])
      AS x(project_id, report_id, stage, source, audience_kind, audience_id, decision_id,
           proposal_id, ai_run_id, workflow_run_id, possible_at)
    ON CONFLICT DO NOTHING";

/// Records exposures in one statement, inside the caller's transaction. An
/// exposure that repeats a decision or proposal identifier for the same source
/// is ignored, so the earliest time is the one kept. Returns the rows written.
pub async fn record_exposures_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    exposures: &[NewExposure],
) -> Result<u64, sqlx::Error> {
    if exposures.is_empty() {
        return Ok(0);
    }
    let mut project_ids = Vec::with_capacity(exposures.len());
    let mut report_ids = Vec::with_capacity(exposures.len());
    let mut stages = Vec::with_capacity(exposures.len());
    let mut sources = Vec::with_capacity(exposures.len());
    let mut audience_kinds = Vec::with_capacity(exposures.len());
    let mut audience_ids = Vec::with_capacity(exposures.len());
    let mut decision_ids = Vec::with_capacity(exposures.len());
    let mut proposal_ids = Vec::with_capacity(exposures.len());
    let mut ai_run_ids = Vec::with_capacity(exposures.len());
    let mut workflow_run_ids = Vec::with_capacity(exposures.len());
    let mut possible_at = Vec::with_capacity(exposures.len());
    for exposure in exposures {
        project_ids.push(exposure.project_id);
        report_ids.push(exposure.report_id);
        stages.push(exposure.stage.clone());
        sources.push(exposure.source.as_str().to_owned());
        audience_kinds.push(
            exposure
                .audience
                .as_ref()
                .map(|actor| actor.kind().as_str().to_owned()),
        );
        audience_ids.push(
            exposure
                .audience
                .as_ref()
                .map(|actor| actor.id().to_owned()),
        );
        decision_ids.push(exposure.ai_reviewer_decision_id);
        proposal_ids.push(exposure.proposal_id);
        ai_run_ids.push(exposure.ai_run_id);
        workflow_run_ids.push(exposure.workflow_run_id);
        possible_at.push(exposure.exposure_possible_at);
    }
    let result = sqlx::query(INSERT_EXPOSURES)
        .bind(project_ids)
        .bind(report_ids)
        .bind(stages)
        .bind(sources)
        .bind(audience_kinds)
        .bind(audience_ids)
        .bind(decision_ids)
        .bind(proposal_ids)
        .bind(ai_run_ids)
        .bind(workflow_run_ids)
        .bind(possible_at)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected())
}

/// Records one exposure inside the caller's transaction.
pub async fn record_exposure_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    exposure: &NewExposure,
) -> Result<u64, sqlx::Error> {
    record_exposures_in_transaction(tx, std::slice::from_ref(exposure)).await
}

/// Records exposures in their own transaction.
pub async fn record_exposures(
    pool: &PgPool,
    exposures: &[NewExposure],
) -> Result<u64, sqlx::Error> {
    if exposures.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let written = record_exposures_in_transaction(&mut tx, exposures).await?;
    tx.commit().await?;
    Ok(written)
}

/// Records one exposure in its own transaction.
pub async fn record_exposure(pool: &PgPool, exposure: &NewExposure) -> Result<u64, sqlx::Error> {
    record_exposures(pool, std::slice::from_ref(exposure)).await
}

/// Records that these opinions were returned to a person: by the reviewer-decision
/// API, or by the activity feed once the person had decided. One statement for the
/// whole batch. A decision that was already recorded keeps its first time.
pub(crate) async fn record_reveals(
    pool: &PgPool,
    decision_ids: &[Uuid],
) -> Result<u64, sqlx::Error> {
    if decision_ids.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        "INSERT INTO ai_opinion_exposures
           (project_id, report_id, stage, exposure_source, ai_reviewer_decision_id,
            proposal_id, ai_run_id, exposure_possible_at)
         SELECT d.project_id, d.report_id, d.stage, 'reviewer_opinion_reveal', d.id,
                d.proposal_id, d.ai_run_id, now()
         FROM ai_reviewer_decisions d
         WHERE d.id = ANY($1)
         ON CONFLICT DO NOTHING",
    )
    .bind(decision_ids)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Records that these screening proposals were listed or fetched. Only screening
/// suggestions count. Other proposals carry no verdict about a record.
pub(crate) async fn record_suggestion_exposures(
    pool: &PgPool,
    project_id: Uuid,
    proposal_ids: &[Uuid],
) -> Result<u64, sqlx::Error> {
    if proposal_ids.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        "INSERT INTO ai_opinion_exposures
           (project_id, report_id, stage, exposure_source, proposal_id, ai_run_id, exposure_possible_at)
         SELECT p.project_id, p.target_report_id, p.payload->>'stage', 'screening_suggestion',
                p.id, p.model_run_id, now()
         FROM ai_proposals p
         WHERE p.project_id=$1 AND p.id = ANY($2)
           AND p.operation='screening_suggestion' AND p.target_report_id IS NOT NULL
           AND p.payload->>'stage' IN ('title_abstract', 'full_text')
         ON CONFLICT DO NOTHING",
    )
    .bind(project_id)
    .bind(proposal_ids)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// True for a screening proposal that is the AI second reviewer's opinion, while
/// the person has not decided that record and stage. It is the SQL condition
/// over `p`, an `ai_proposals` row, and it withholds the proposal from every
/// reader except the internal workflow and autonomy paths. During a blind audit,
/// all stages are withheld so a prior full-text opinion cannot identify controls.
pub(crate) const PROPOSAL_WITHHELD_SQL: &str = "(ai_first_audit_masked(p.project_id, p.target_report_id) OR ai_screening_proposal_withheld(p.project_id, p.id, p.target_report_id, p.payload->>'stage'))";

/// A human decision and the AI second reviewer's opinion on the same record and
/// stage. The pair stands as long as the person's decision is still the one it
/// was compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerPair {
    pub decision_id: Uuid,
    pub report_id: Uuid,
    pub stage: String,
    /// `include`, `exclude` or `maybe`: the person's decision, before any conflict was settled.
    pub human_decision: String,
    /// `include`, `exclude` or `maybe`.
    pub ai_decision: String,
}

/// Pairs that count as independent evidence, and how many were left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewerPairs {
    /// Pairs the person decided without the AI opinion being visible.
    pub independent: Vec<ReviewerPair>,
    /// Pairs left out because an AI opinion was visible before the person's decision.
    pub exposed_excluded: i64,
    /// Pairs left out because when the person decided is not on record, so
    /// independence cannot be shown.
    pub unverifiable_excluded: i64,
}

/// Every non-voided AI second-reviewer opinion that a person has also decided,
/// split into independent pairs and excluded ones. One query, with no row cap.
///
/// A pair is excluded as exposed when an exposure for the same record and stage
/// is at or before the person's decision time. An exposure with an audience
/// counts only for that actor. A pair whose decision time is unknown is
/// excluded as unverifiable. For a settled conflict, the decision that counts
/// is the one that was replaced, and its time is stored at resolution.
pub async fn independent_reviewer_pairs(
    pool: &PgPool,
    project_id: Uuid,
    stage: Option<&str>,
) -> Result<ReviewerPairs, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT t.decision_id, t.report_id, t.stage, t.ai_decision, t.human_decision,
                EXISTS (
                  SELECT 1 FROM ai_opinion_exposures o
                  WHERE o.project_id=t.project_id AND o.report_id=t.report_id AND o.stage=t.stage
                    AND o.exposure_possible_at <= t.human_at
                    AND (o.audience_actor_id IS NULL OR t.actor_id IS NULL
                         OR (o.audience_actor_kind=t.actor_kind AND o.audience_actor_id=t.actor_id))
                ) AS exposed,
                t.human_at IS NULL AS unverifiable
         FROM (
           SELECT p.id AS decision_id, p.project_id, p.report_id, p.stage,
                  p.decision AS ai_decision, p.human_decision,
                  CASE WHEN h.actor_kind='user' AND h.event_kind='decision'
                              AND h.decision=p.human_decision AND NOT h.audit_reference
                       THEN CASE WHEN p.resolved_at IS NOT NULL THEN p.human_decision_before_at
                                 ELSE h.created_at END END AS human_at,
                  h.actor_kind, h.actor_id
           FROM (
             SELECT d.id, d.project_id, d.report_id, d.stage, d.decision, d.resolved_at,
                    d.human_decision_before_at,
                    CASE WHEN d.resolved_at IS NOT NULL THEN d.human_decision_before
                         ELSE CASE WHEN d.stage='full_text' THEN s.full_text_status
                                   ELSE s.title_abstract_status END
                    END AS human_decision
             FROM ai_reviewer_decisions d
             LEFT JOIN screening_state s ON s.project_id=d.project_id AND s.report_id=d.report_id
             WHERE d.project_id=$1 AND d.voided_at IS NULL AND ($2::text IS NULL OR d.stage=$2)
           ) p
           LEFT JOIN LATERAL (
             SELECT e.created_at, e.actor_kind, e.actor_id, e.event_kind, e.decision,
               (e.notes LIKE '%dual_or%' AND e.notes LIKE '%ai_first_cohort%') IS TRUE AS audit_reference
             FROM screening_events e
             WHERE e.project_id=p.project_id AND e.report_id=p.report_id AND e.stage=p.stage
               AND (p.resolved_at IS NULL OR e.created_at=p.human_decision_before_at)
             ORDER BY e.created_at DESC, e.id DESC
             LIMIT 1
           ) h ON true
           WHERE p.human_decision IS NOT NULL
             AND p.human_decision NOT IN ('unscreened', 'not_required')
         ) t
         ORDER BY t.report_id, t.stage, t.decision_id",
    )
    .bind(project_id)
    .bind(stage)
    .fetch_all(pool)
    .await?;
    let mut pairs = ReviewerPairs::default();
    for row in rows {
        if row.get::<bool, _>("unverifiable") {
            pairs.unverifiable_excluded += 1;
        } else if row.get::<bool, _>("exposed") {
            pairs.exposed_excluded += 1;
        } else {
            pairs.independent.push(ReviewerPair {
                decision_id: row.get("decision_id"),
                report_id: row.get("report_id"),
                stage: row.get("stage"),
                human_decision: row.get("human_decision"),
                ai_decision: row.get("ai_decision"),
            });
        }
    }
    Ok(pairs)
}

/// The proposal and AI run that a screening review run produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewRunProposal {
    /// The review run, as the workflow's wait state names it.
    pub run_id: Uuid,
    pub proposal_id: Uuid,
    pub ai_run_id: Uuid,
}

/// The proposals that the given review runs produced. A run without a proposal
/// is absent from the result.
pub async fn review_run_proposals(
    pool: &PgPool,
    project_id: Uuid,
    run_ids: &[Uuid],
) -> Result<Vec<ReviewRunProposal>, sqlx::Error> {
    if run_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT m.automation_run_id, m.proposal_id, p.model_run_id
         FROM review_run_manifests m
         JOIN ai_proposals p ON p.project_id=m.project_id AND p.id=m.proposal_id
         WHERE m.project_id=$1 AND m.automation_run_id = ANY($2) AND m.proposal_id IS NOT NULL",
    )
    .bind(project_id)
    .bind(run_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| ReviewRunProposal {
            run_id: row.get("automation_run_id"),
            proposal_id: row.get("proposal_id"),
            ai_run_id: row.get("model_run_id"),
        })
        .collect())
}
