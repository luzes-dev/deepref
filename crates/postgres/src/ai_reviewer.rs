//! The AI as an independent second reviewer. Its opinion is stored beside the
//! human screening state and never changes it. When the human decision for the
//! same stage differs, the pair is a conflict that a person resolves.

use chrono::{DateTime, Utc};
use deepref_application::ScreenReportCommand;
use deepref_domain::{Actor, ScreeningDecision, ScreeningStage};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ReviewerError {
    #[error("the AI reviewer decision was not found")]
    NotFound,
    #[error("this conflict was already resolved")]
    AlreadyResolved,
    #[error("a published protocol is required before screening")]
    NoProtocol,
    #[error(transparent)]
    Screening(#[from] crate::ScreeningError),
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
pub struct NewReviewerDecision {
    pub id: Uuid,
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub stage: String,
    pub decision: String,
    pub rationale: String,
    pub evidence: Value,
    pub source: &'static str,
    pub proposal_id: Option<Uuid>,
    pub ai_run_id: Option<Uuid>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub activity_id: Option<Uuid>,
}

/// Stores the AI's opinion, replacing (voiding) its previous one for the same
/// report and stage.
pub async fn insert_reviewer_decision_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    decision: &NewReviewerDecision,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE ai_reviewer_decisions SET voided_at=now()
         WHERE project_id=$1 AND report_id=$2 AND stage=$3 AND voided_at IS NULL",
    )
    .bind(decision.project_id)
    .bind(decision.report_id)
    .bind(&decision.stage)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO ai_reviewer_decisions
         (id,project_id,report_id,stage,decision,rationale,evidence,source,proposal_id,ai_run_id,
          model,prompt_version,activity_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(decision.id)
    .bind(decision.project_id)
    .bind(decision.report_id)
    .bind(&decision.stage)
    .bind(&decision.decision)
    .bind(&decision.rationale)
    .bind(&decision.evidence)
    .bind(decision.source)
    .bind(decision.proposal_id)
    .bind(decision.ai_run_id)
    .bind(&decision.model)
    .bind(&decision.prompt_version)
    .bind(decision.activity_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ReviewerDecisionRecord {
    pub id: Uuid,
    pub report_id: Uuid,
    pub stage: String,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub ai_decision: String,
    pub ai_rationale: String,
    pub ai_evidence: Value,
    pub ai_model: Option<String>,
    pub ai_source: String,
    pub human_decision: Option<String>,
    pub human_notes: Option<String>,
    pub human_actor: Option<String>,
    /// `waiting`, `concordant`, `conflict` or `resolved`.
    pub status: String,
    pub resolution: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const STATUS_VIEW: &str = "
    SELECT d.id,d.report_id,d.stage,d.decision AS ai_decision,d.rationale,d.evidence,d.model,
           d.source,d.created_at,d.resolved_at,d.resolution,d.human_decision_before,
           r.title,r.abstract_text,
           CASE d.stage WHEN 'title_abstract' THEN s.title_abstract_status
                        ELSE s.full_text_status END AS human_status,
           e.notes AS human_notes,
           CASE WHEN e.event_kind='decision' THEN e.actor_id END AS human_actor
    FROM ai_reviewer_decisions d
    JOIN reports r ON r.id=d.report_id
    LEFT JOIN screening_state s ON s.project_id=d.project_id AND s.report_id=d.report_id
    LEFT JOIN screening_events e ON e.id=s.last_event_id
    WHERE d.project_id=$1 AND d.voided_at IS NULL AND ($2::text IS NULL OR d.stage=$2)";

fn status_of(
    resolved_at: Option<DateTime<Utc>>,
    human_status: Option<&str>,
    ai_decision: &str,
) -> &'static str {
    if resolved_at.is_some() {
        return "resolved";
    }
    match human_status {
        None | Some("unscreened" | "not_required") => "waiting",
        Some(status) if status == ai_decision => "concordant",
        Some(_) => "conflict",
    }
}

fn record_from_row(row: &sqlx::postgres::PgRow) -> ReviewerDecisionRecord {
    let ai_decision: String = row.get("ai_decision");
    let human_status: Option<String> = row.get("human_status");
    let resolved_at: Option<DateTime<Utc>> = row.get("resolved_at");
    let status = status_of(resolved_at, human_status.as_deref(), &ai_decision);
    // Blinding: until the person has decided, the AI's opinion is withheld from
    // every reader. Second-reviewer independence depends on it, so the server
    // never returns the decision, rationale or evidence for a waiting record.
    let withheld = status == "waiting";
    let human_decision = match human_status.as_deref() {
        None | Some("unscreened" | "not_required") => None,
        Some(value) => Some(value.to_owned()),
    };
    ReviewerDecisionRecord {
        id: row.get("id"),
        report_id: row.get("report_id"),
        stage: row.get("stage"),
        title: row.get("title"),
        abstract_text: row.get("abstract_text"),
        ai_decision: if withheld { String::new() } else { ai_decision },
        ai_rationale: if withheld {
            String::new()
        } else {
            row.get("rationale")
        },
        ai_evidence: if withheld {
            Value::Array(Vec::new())
        } else {
            row.get("evidence")
        },
        ai_model: if withheld { None } else { row.get("model") },
        ai_source: row.get("source"),
        human_decision,
        human_notes: row.get("human_notes"),
        human_actor: row.get("human_actor"),
        status: status.to_owned(),
        resolution: row.get("resolution"),
        resolved_at,
        created_at: row.get("created_at"),
    }
}

pub async fn list_reviewer_decisions(
    pool: &PgPool,
    project_id: Uuid,
    stage: Option<&str>,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<ReviewerDecisionRecord>, sqlx::Error> {
    let query = format!("{STATUS_VIEW} ORDER BY d.created_at DESC,d.id DESC LIMIT 500");
    let rows = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(project_id)
        .bind(stage)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(record_from_row)
        .filter(|record| status.is_none_or(|wanted| record.status == wanted))
        .take(usize::try_from(limit).unwrap_or(100))
        .collect())
}

pub(crate) async fn count_open_conflicts(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<i64, sqlx::Error> {
    Ok(
        list_reviewer_decisions(pool, project_id, None, Some("conflict"), 100_000)
            .await?
            .len() as i64,
    )
}

#[derive(Debug, Clone)]
pub struct ResolveConflict {
    pub decision: ScreeningDecision,
    pub exclusion_reason_id: Option<Uuid>,
    pub note: Option<String>,
}

fn decision_name(decision: ScreeningDecision) -> &'static str {
    match decision {
        ScreeningDecision::Include => "include",
        ScreeningDecision::Exclude => "exclude",
        ScreeningDecision::Maybe => "maybe",
    }
}

/// A person settles a disagreement: their final decision is recorded through
/// the normal screening service and the conflict is marked resolved.
pub async fn resolve_reviewer_conflict(
    pool: &PgPool,
    project_id: Uuid,
    decision_id: Uuid,
    input: ResolveConflict,
    actor: &Actor,
) -> Result<ReviewerDecisionRecord, ReviewerError> {
    let row = sqlx::query(
        "SELECT d.report_id,d.stage,d.decision,d.resolved_at,
                CASE d.stage WHEN 'title_abstract' THEN s.title_abstract_status
                             ELSE s.full_text_status END AS human_status,
                COALESCE(s.revision,0) AS revision
         FROM ai_reviewer_decisions d
         LEFT JOIN screening_state s ON s.project_id=d.project_id AND s.report_id=d.report_id
         WHERE d.project_id=$1 AND d.id=$2 AND d.voided_at IS NULL",
    )
    .bind(project_id)
    .bind(decision_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ReviewerError::NotFound)?;
    if row.get::<Option<DateTime<Utc>>, _>("resolved_at").is_some() {
        return Err(ReviewerError::AlreadyResolved);
    }
    let report_id: Uuid = row.get("report_id");
    let stage_name: String = row.get("stage");
    let ai_decision: String = row.get("decision");
    let human_before: Option<String> = row
        .get::<Option<String>, _>("human_status")
        .filter(|status| !matches!(status.as_str(), "unscreened" | "not_required"));
    let stage = if stage_name == "full_text" {
        ScreeningStage::FullText
    } else {
        ScreeningStage::TitleAbstract
    };
    let protocol = crate::get_published_protocol(pool, project_id)
        .await
        .map_err(|_| ReviewerError::NoProtocol)?;
    let notes = Some(
        match input
            .note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            Some(note) => format!("{note} [resolved AI conflict]"),
            None => "Resolved a disagreement with the AI reviewer".to_owned(),
        },
    );
    let result = crate::screening::screen_report(
        pool,
        ScreenReportCommand {
            project_id: project_id.into(),
            report_id: report_id.into(),
            stage,
            decision: input.decision,
            exclusion_reason_id: input.exclusion_reason_id.map(Into::into),
            protocol_version_id: protocol.id.into(),
            expected_revision: row.get("revision"),
            notes,
            actor: actor.clone(),
        },
    )
    .await;
    match result {
        Ok(_) | Err(crate::ScreeningError::Repeated { .. }) => {}
        Err(error) => return Err(error.into()),
    }
    let chosen = decision_name(input.decision);
    let resolution = if chosen == ai_decision {
        "adopted_ai"
    } else if human_before.as_deref() == Some(chosen) {
        "kept_human"
    } else {
        "other"
    };
    sqlx::query(
        "UPDATE ai_reviewer_decisions
         SET resolved_at=now(),resolved_by_kind=$3,resolved_by_id=$4,resolution=$5,
             resolution_note=$6,human_decision_before=$7
         WHERE project_id=$1 AND id=$2 AND resolved_at IS NULL",
    )
    .bind(project_id)
    .bind(decision_id)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .bind(resolution)
    .bind(&input.note)
    .bind(&human_before)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO review_events
         (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
         VALUES ($1,$2,'ai_conflict_resolved','ai_reviewer_decision',$3,$4,$5,$6)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(decision_id)
    .bind(serde_json::json!({
        "report_id": report_id, "stage": stage_name, "ai_decision": ai_decision,
        "human_before": human_before, "final": chosen, "resolution": resolution,
    }))
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(pool)
    .await?;
    let query = format!("{STATUS_VIEW} AND d.id=$3");
    let row = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(project_id)
        .bind(Option::<String>::None)
        .bind(decision_id)
        .fetch_one(pool)
        .await?;
    Ok(record_from_row(&row))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageAgreement {
    pub compared: i64,
    pub agreed: i64,
    pub kappa: Option<f64>,
}

/// Cohen's kappa over include / exclude / maybe for matching decisions.
pub fn cohens_kappa(pairs: &[(usize, usize)]) -> Option<f64> {
    let n = pairs.len() as f64;
    if pairs.is_empty() {
        return None;
    }
    let mut matrix = [[0.0_f64; 3]; 3];
    for (human, ai) in pairs {
        matrix[*human][*ai] += 1.0;
    }
    let observed = (0..3).map(|i| matrix[i][i]).sum::<f64>() / n;
    let expected = (0..3)
        .map(|i| {
            let row: f64 = matrix[i].iter().sum();
            let column: f64 = (0..3).map(|j| matrix[j][i]).sum();
            (row / n) * (column / n)
        })
        .sum::<f64>();
    if (1.0 - expected).abs() < f64::EPSILON {
        return None;
    }
    Some((observed - expected) / (1.0 - expected))
}

fn category(value: &str) -> Option<usize> {
    match value {
        "include" => Some(0),
        "exclude" => Some(1),
        "maybe" => Some(2),
        _ => None,
    }
}

/// Human-versus-AI agreement per stage, using each person's independent
/// decision (before any conflict was settled).
pub async fn reviewer_agreement(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<Vec<(String, StageAgreement)>, sqlx::Error> {
    let rows = list_reviewer_decisions(pool, project_id, None, None, 100_000).await?;
    let mut result = Vec::new();
    for stage in ["title_abstract", "full_text"] {
        let pairs: Vec<(usize, usize)> = rows
            .iter()
            .filter(|row| row.stage == stage && row.status != "waiting")
            .filter_map(|row| {
                let human = match row.status.as_str() {
                    "resolved" => None,
                    _ => row.human_decision.as_deref(),
                };
                Some((category(human?)?, category(&row.ai_decision)?))
            })
            .collect();
        // Resolved rows use the stored pre-resolution human decision.
        let resolved: Vec<(usize, usize)> = sqlx::query(
            "SELECT human_decision_before,decision FROM ai_reviewer_decisions
             WHERE project_id=$1 AND stage=$2 AND voided_at IS NULL
               AND resolved_at IS NOT NULL AND human_decision_before IS NOT NULL",
        )
        .bind(project_id)
        .bind(stage)
        .fetch_all(pool)
        .await?
        .iter()
        .filter_map(|row| {
            Some((
                category(&row.get::<String, _>("human_decision_before"))?,
                category(&row.get::<String, _>("decision"))?,
            ))
        })
        .collect();
        let all: Vec<(usize, usize)> = pairs.into_iter().chain(resolved).collect();
        result.push((
            stage.to_owned(),
            StageAgreement {
                compared: all.len() as i64,
                agreed: all.iter().filter(|(a, b)| a == b).count() as i64,
                kappa: cohens_kappa(&all),
            },
        ));
    }
    Ok(result)
}

/// An automation's opinion recorded as the second reviewer (it never changes
/// the screening state). Writes an activity entry that can be undone.
#[allow(clippy::too_many_arguments)]
pub async fn record_workflow_reviewer_decision(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    stage: &str,
    decision: &str,
    actor: &Actor,
    note: Option<&str>,
    batch_id: Option<Uuid>,
) -> Result<Uuid, sqlx::Error> {
    let title: String =
        sqlx::query_scalar::<_, Option<String>>("SELECT title FROM reports WHERE id=$1")
            .bind(report_id)
            .fetch_optional(pool)
            .await?
            .flatten()
            .unwrap_or_else(|| "Untitled record".to_owned());
    let decision_id = Uuid::new_v4();
    let shown: String = title.chars().take(120).collect();
    let mut entry = crate::activity::NewActivity::new(
        project_id,
        "automation",
        "Automation",
        actor.clone(),
        if stage == "full_text" {
            "full_text_screening"
        } else {
            "title_abstract_screening"
        },
        "second_reviewer_opinion",
        format!(
            "An automation recorded “{decision}” for “{shown}” as a second opinion. Your own decision is still needed."
        ),
    );
    entry.affected = serde_json::json!([{"type": "report", "id": report_id, "label": title}]);
    entry.after_state = serde_json::json!({"decision_id": decision_id});
    entry.undo_kind = Some("reviewer_decision");
    entry.batch_id = batch_id;
    let mut tx = pool.begin().await?;
    let activity_id = crate::activity::record_activity_in_transaction(&mut tx, &entry).await?;
    insert_reviewer_decision_in_transaction(
        &mut tx,
        &NewReviewerDecision {
            id: decision_id,
            project_id,
            report_id,
            stage: stage.to_owned(),
            decision: decision.to_owned(),
            rationale: note
                .map(str::to_owned)
                .unwrap_or_else(|| "Recorded by an automation.".to_owned()),
            evidence: serde_json::json!([]),
            source: "workflow",
            proposal_id: None,
            ai_run_id: None,
            model: None,
            prompt_version: None,
            activity_id: Some(activity_id),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(decision_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kappa_is_one_for_perfect_agreement_and_zero_for_chance() {
        let perfect = [(0, 0), (1, 1), (0, 0), (1, 1)];
        assert!((cohens_kappa(&perfect).unwrap() - 1.0).abs() < 1e-9);
        let chance = [(0, 0), (0, 1), (1, 0), (1, 1)];
        assert!(cohens_kappa(&chance).unwrap().abs() < 1e-9);
        assert_eq!(cohens_kappa(&[]), None);
        assert_eq!(cohens_kappa(&[(0, 0), (0, 0)]), None);
    }

    #[test]
    fn status_reflects_the_human_decision() {
        assert_eq!(status_of(None, Some("unscreened"), "include"), "waiting");
        assert_eq!(status_of(None, Some("include"), "include"), "concordant");
        assert_eq!(status_of(None, Some("exclude"), "include"), "conflict");
        assert_eq!(
            status_of(Some(Utc::now()), Some("exclude"), "include"),
            "resolved"
        );
    }
}
