//! Applies the project's AI autonomy settings to a freshly persisted AI
//! proposal: leave it as a suggestion, record it as the AI second reviewer's
//! independent opinion, or act on it and write an undoable activity entry.

use deepref_application::workflows::{AutonomyLevel, AutonomyTask};
use deepref_domain::{Actor, ActorKind};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    activity::{NewActivity, record_activity_in_transaction},
    ai::{AiProposalDecision, AiProposalDecisionRequest, AiProposalRecord, decide_ai_proposal},
    ai_reviewer::{NewReviewerDecision, insert_reviewer_decision_in_transaction},
    autonomy::{resolve_autonomy_level, task_for_proposal},
    extraction::{ExtractionApplyOptions, apply_data_extraction_in_transaction},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomyOutcome {
    /// Nothing automatic happened; a person decides in the proposal queue.
    LeftAsSuggestion,
    /// Recorded as the AI's independent screening opinion.
    SecondReviewer { activity_id: Uuid },
    /// Applied and recorded in the activity feed.
    Applied { activity_id: Uuid },
    /// Nothing to do (for example every field already had a value).
    Skipped(&'static str),
}

fn ai_actor(model: &str) -> anyhow::Result<Actor> {
    Ok(Actor::new(ActorKind::Automation, format!("ai:{model}"))?)
}

fn truncate(text: &str, max: usize) -> String {
    let mut shortened: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        shortened.push('…');
    }
    shortened
}

/// Entry point called after a proposal was committed.
pub async fn apply_autonomy_for_proposal(
    pool: &PgPool,
    project_id: Uuid,
    proposal_id: Uuid,
) -> anyhow::Result<AutonomyOutcome> {
    let proposal = crate::ai::get_ai_proposal(pool, project_id, proposal_id).await?;
    if proposal.status != "pending" {
        return Ok(AutonomyOutcome::LeftAsSuggestion);
    }
    let Some(task) = task_for_proposal(&proposal.operation, &proposal.payload) else {
        return Ok(AutonomyOutcome::LeftAsSuggestion);
    };
    let level = resolve_autonomy_level(pool, project_id, task).await?;
    match (task, level) {
        (
            AutonomyTask::TitleAbstractScreening | AutonomyTask::FullTextScreening,
            AutonomyLevel::SecondReviewer,
        ) => record_second_reviewer(pool, &proposal, task).await,
        (AutonomyTask::Extraction, AutonomyLevel::Act) => act_on_extraction(pool, &proposal).await,
        (AutonomyTask::FuzzyDuplicates, AutonomyLevel::Act) => {
            act_on_duplicate(pool, &proposal).await
        }
        _ => Ok(AutonomyOutcome::LeftAsSuggestion),
    }
}

async fn report_title(pool: &PgPool, report_id: Uuid) -> String {
    sqlx::query_scalar::<_, Option<String>>("SELECT title FROM reports WHERE id=$1")
        .bind(report_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .unwrap_or_else(|| "Untitled record".to_owned())
}

async fn resolve_proposal_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    proposal: &AiProposalRecord,
    status: &str,
    actor: &Actor,
    reason: &str,
) -> anyhow::Result<()> {
    let updated = sqlx::query(
        "UPDATE ai_proposals
         SET status=$3,resolved_at=now(),resolved_by_actor_kind=$4,resolved_by_actor_id=$5,
             resolution_reason=$6,decided_by=$5,decided_at=now()
         WHERE project_id=$1 AND id=$2 AND status='pending'",
    )
    .bind(proposal.project_id)
    .bind(proposal.id)
    .bind(status)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        anyhow::bail!("the proposal was resolved by someone else first");
    }
    sqlx::query(
        "INSERT INTO review_events
         (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
         VALUES ($1,$2,'ai_proposal_resolved','ai_proposal',$3,$4,$5,$6)",
    )
    .bind(Uuid::new_v4())
    .bind(proposal.project_id)
    .bind(proposal.id)
    .bind(json!({"status": status, "operation": proposal.operation, "automatic": true}))
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Second reviewer
// ---------------------------------------------------------------------------

async fn screening_opinion(
    pool: &PgPool,
    proposal: &AiProposalRecord,
    report_id: Uuid,
) -> anyhow::Result<(String, Value)> {
    let criteria: Vec<Value> = proposal
        .payload
        .get("criteria")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ids: Vec<Uuid> = criteria
        .iter()
        .filter_map(|item| {
            item.get("criterion_id")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
        })
        .collect();
    let labels: Vec<(Uuid, String)> =
        sqlx::query("SELECT id,label FROM eligibility_criteria WHERE id=ANY($1)")
            .bind(&ids)
            .fetch_all(pool)
            .await?
            .iter()
            .map(|row| (row.get("id"), row.get("label")))
            .collect();

    let mut parts = Vec::new();
    let mut evidence = Vec::new();
    for item in &criteria {
        let judgment = match item.get("judgment").and_then(Value::as_str) {
            Some("meets") => "meets",
            Some("does_not_meet") => "does not meet",
            _ => "unclear",
        };
        let label = item
            .get("criterion_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
            .and_then(|id| {
                labels
                    .iter()
                    .find(|(candidate, _)| *candidate == id)
                    .map(|(_, label)| label.clone())
            })
            .unwrap_or_else(|| "Criterion".to_owned());
        let rationale = item
            .get("rationale")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if parts.len() < 4 {
            parts.push(format!("{label} ({judgment}): {rationale}"));
        }
        for entry in item
            .get("evidence")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if evidence.len() >= 4 {
                break;
            }
            let quote = match entry.get("kind").and_then(Value::as_str) {
                Some("report_metadata") => {
                    let field = entry
                        .get("field")
                        .and_then(Value::as_str)
                        .unwrap_or("title");
                    let column = if field == "abstract" {
                        "abstract_text"
                    } else {
                        "title"
                    };
                    let text: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                        "SELECT {column} FROM reports WHERE id=$1"
                    )))
                    .bind(report_id)
                    .fetch_optional(pool)
                    .await?
                    .flatten();
                    text.map(|text| {
                        (
                            if field == "abstract" {
                                "Abstract"
                            } else {
                                "Title"
                            }
                            .to_owned(),
                            text,
                        )
                    })
                }
                Some("document_block") => {
                    let block = entry
                        .get("document_block_id")
                        .and_then(Value::as_str)
                        .and_then(|id| Uuid::parse_str(id).ok());
                    let page = entry.get("page").and_then(Value::as_u64).unwrap_or(0);
                    match block {
                        Some(block) => sqlx::query_scalar::<_, String>(
                            "SELECT text FROM document_blocks WHERE id=$1",
                        )
                        .bind(block)
                        .fetch_optional(pool)
                        .await?
                        .map(|text| (format!("Full text, page {page}"), text)),
                        None => None,
                    }
                }
                _ => None,
            };
            if let Some((label, text)) = quote {
                let quote = truncate(text.trim(), 400);
                if !quote.is_empty()
                    && !evidence
                        .iter()
                        .any(|existing: &Value| existing["quote"] == json!(quote))
                {
                    evidence.push(json!({"label": label, "quote": quote}));
                }
            }
        }
    }
    let rationale = if parts.is_empty() {
        "The AI gave no detailed rationale.".to_owned()
    } else {
        truncate(&parts.join("\n"), 2_000)
    };
    Ok((rationale, Value::Array(evidence)))
}

async fn record_second_reviewer(
    pool: &PgPool,
    proposal: &AiProposalRecord,
    task: AutonomyTask,
) -> anyhow::Result<AutonomyOutcome> {
    let Some(report_id) = proposal.target_report_id else {
        return Ok(AutonomyOutcome::LeftAsSuggestion);
    };
    let decision = match proposal
        .payload
        .get("suggested_decision")
        .and_then(|value| value.get("kind"))
        .and_then(Value::as_str)
        .and_then(deepref_application::workflows::autonomy::second_reviewer_opinion)
    {
        Some(opinion) => opinion.to_owned(),
        // "Not enough evidence" is not an opinion; keep it as a suggestion.
        None => return Ok(AutonomyOutcome::LeftAsSuggestion),
    };
    let stage = if task == AutonomyTask::FullTextScreening {
        "full_text"
    } else {
        "title_abstract"
    };
    let (rationale, evidence) = screening_opinion(pool, proposal, report_id).await?;
    let title = report_title(pool, report_id).await;
    let actor = ai_actor(&proposal.model)?;
    let decision_id = Uuid::new_v4();
    let verb = match decision.as_str() {
        "include" => "would include",
        "exclude" => "would exclude",
        _ => "is unsure about",
    };
    let mut entry = NewActivity::new(
        proposal.project_id,
        "ai",
        proposal.model.clone(),
        actor.clone(),
        task.as_str(),
        "second_reviewer_opinion",
        format!(
            "The AI, as second reviewer, {verb} “{}”. Your own decision is still needed.",
            truncate(&title, 120)
        ),
    );
    entry.affected = json!([{"type": "report", "id": report_id, "label": title}]);
    entry.after_state = json!({"decision_id": decision_id});
    entry.undo_kind = Some("reviewer_decision");
    entry.ai_run_id = Some(proposal.model_run_id);
    entry.proposal_id = Some(proposal.id);
    entry.model = Some(proposal.model.clone());
    entry.prompt_version = Some(proposal.prompt_version.clone());
    entry.evidence = evidence.clone();

    let mut tx = pool.begin().await?;
    let activity_id = record_activity_in_transaction(&mut tx, &entry).await?;
    insert_reviewer_decision_in_transaction(
        &mut tx,
        &NewReviewerDecision {
            id: decision_id,
            project_id: proposal.project_id,
            report_id,
            stage: stage.to_owned(),
            decision,
            rationale,
            evidence,
            source: "ai",
            proposal_id: Some(proposal.id),
            ai_run_id: Some(proposal.model_run_id),
            model: Some(proposal.model.clone()),
            prompt_version: Some(proposal.prompt_version.clone()),
            activity_id: Some(activity_id),
        },
    )
    .await?;
    resolve_proposal_in_transaction(
        &mut tx,
        proposal,
        "expired",
        &actor,
        "Recorded as the AI second reviewer's independent opinion",
    )
    .await?;
    tx.commit().await?;
    Ok(AutonomyOutcome::SecondReviewer { activity_id })
}

// ---------------------------------------------------------------------------
// Extraction: act and notify
// ---------------------------------------------------------------------------

async fn act_on_extraction(
    pool: &PgPool,
    proposal: &AiProposalRecord,
) -> anyhow::Result<AutonomyOutcome> {
    let extraction: deepref_ai::DataExtraction = serde_json::from_value(proposal.payload.clone())?;
    let Some(study_id) = proposal.target_study_id else {
        return Ok(AutonomyOutcome::LeftAsSuggestion);
    };
    // Only values that cite a source passage may be stored automatically.
    let cited = extraction
        .fields
        .iter()
        .filter(|field| {
            matches!(
                field,
                deepref_ai::ExtractedField::Value { source, .. }
                    if !source.content_hash.is_empty() && source.page > 0
            )
        })
        .count();
    if cited == 0 {
        return Ok(AutonomyOutcome::LeftAsSuggestion);
    }
    let actor = ai_actor(&proposal.model)?;
    let mut tx = pool.begin().await?;
    let applied = apply_data_extraction_in_transaction(
        &mut tx,
        proposal.project_id.into(),
        study_id,
        proposal.id,
        &extraction,
        &actor,
        ExtractionApplyOptions {
            needs_verification: true,
            skip_existing: true,
        },
    )
    .await;
    let applied = match applied {
        Ok(applied) => applied,
        Err(error) => {
            // Anything the normal service rejects stays a suggestion.
            tracing::warn!(%error, proposal = %proposal.id, "automatic extraction fell back to a suggestion");
            let _ = tx.rollback().await;
            return Ok(AutonomyOutcome::LeftAsSuggestion);
        }
    };
    if applied.inserted_value_ids.is_empty() {
        resolve_proposal_in_transaction(
            &mut tx,
            proposal,
            "expired",
            &actor,
            "Every field already had a value, so nothing was changed",
        )
        .await?;
        tx.commit().await?;
        return Ok(AutonomyOutcome::Skipped("every field already had a value"));
    }
    let study_title: String = sqlx::query_scalar("SELECT title FROM studies WHERE id=$1")
        .bind(study_id)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or_else(|| "a study".to_owned());
    let value_rows = sqlx::query(
        "SELECT v.id,d.label,v.source_block_id,v.source_page,v.rationale
         FROM extraction_values v
         JOIN extraction_field_definitions d ON d.project_id=v.project_id
           AND d.id=v.field_definition_id AND d.version=v.field_definition_version
         WHERE v.id=ANY($1)",
    )
    .bind(&applied.inserted_value_ids)
    .fetch_all(&mut *tx)
    .await?;
    let mut affected = vec![json!({"type": "study", "id": study_id, "label": study_title})];
    let mut evidence = Vec::new();
    for row in &value_rows {
        let label: String = row.get("label");
        affected.push(json!({
            "type": "extraction_value",
            "id": row.get::<Uuid, _>("id"),
            "label": label,
        }));
        let block: Option<Uuid> = row.get("source_block_id");
        let quote: Option<String> = match block {
            Some(block) => {
                sqlx::query_scalar("SELECT text FROM document_blocks WHERE id=$1")
                    .bind(block)
                    .fetch_optional(&mut *tx)
                    .await?
            }
            None => None,
        };
        evidence.push(json!({
            "label": format!(
                "{label}, page {}",
                row.get::<Option<i32>, _>("source_page").unwrap_or(0)
            ),
            "quote": truncate(quote.unwrap_or_default().trim(), 400),
        }));
    }
    let count = applied.inserted_value_ids.len();
    let mut entry = NewActivity::new(
        proposal.project_id,
        "ai",
        proposal.model.clone(),
        actor.clone(),
        AutonomyTask::Extraction.as_str(),
        "extraction_values_added",
        format!(
            "The AI filled {count} {} for “{}” from the full text. {} to verify.",
            if count == 1 { "field" } else { "fields" },
            truncate(&study_title, 120),
            if count == 1 {
                "It is marked"
            } else {
                "They are marked"
            },
        ),
    );
    entry.affected = Value::Array(affected);
    entry.after_state = json!({"value_ids": applied.inserted_value_ids, "study_id": study_id});
    entry.undo_kind = Some("extraction_values");
    entry.ai_run_id = Some(proposal.model_run_id);
    entry.proposal_id = Some(proposal.id);
    entry.model = Some(proposal.model.clone());
    entry.prompt_version = Some(proposal.prompt_version.clone());
    entry.evidence = Value::Array(evidence);
    let activity_id = record_activity_in_transaction(&mut tx, &entry).await?;
    sqlx::query("UPDATE extraction_values SET activity_id=$2 WHERE id=ANY($1)")
        .bind(&applied.inserted_value_ids)
        .bind(activity_id)
        .execute(&mut *tx)
        .await?;
    resolve_proposal_in_transaction(
        &mut tx,
        proposal,
        "accepted",
        &actor,
        "Applied automatically; values are marked to verify",
    )
    .await?;
    tx.commit().await?;
    Ok(AutonomyOutcome::Applied { activity_id })
}

// ---------------------------------------------------------------------------
// Duplicates: act and notify
// ---------------------------------------------------------------------------

/// A model-suggested merge is applied on its own only when every signal agrees
/// and the titles are near-identical.
fn duplicate_is_confident(payload: &Value) -> bool {
    if payload.get("decision").and_then(Value::as_str) != Some("match") {
        return false;
    }
    let signals = payload
        .get("signals")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let title_ok = signals.iter().any(|signal| {
        signal.get("kind").and_then(Value::as_str) == Some("title_similarity")
            && signal
                .get("similarity")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                >= 0.95
    });
    let contradicted = signals
        .iter()
        .any(|signal| signal.get("supports_match") == Some(&Value::Bool(false)));
    title_ok && !contradicted
}

async fn act_on_duplicate(
    pool: &PgPool,
    proposal: &AiProposalRecord,
) -> anyhow::Result<AutonomyOutcome> {
    if !duplicate_is_confident(&proposal.payload) {
        return Ok(AutonomyOutcome::LeftAsSuggestion);
    }
    let Some(record_id) = proposal.target_record_id else {
        return Ok(AutonomyOutcome::LeftAsSuggestion);
    };
    let actor = ai_actor(&proposal.model)?;
    let title: String = sqlx::query_scalar::<_, Option<String>>(
        "SELECT title FROM records WHERE project_id=$1 AND id=$2",
    )
    .bind(proposal.project_id)
    .bind(record_id)
    .fetch_optional(pool)
    .await?
    .flatten()
    .unwrap_or_else(|| "Untitled record".to_owned());
    let decided = decide_ai_proposal(
        pool,
        AiProposalDecisionRequest {
            project_id: proposal.project_id,
            proposal_id: proposal.id,
            decision: AiProposalDecision::Accept,
            reason: "Merged automatically: the titles are near-identical and no signal disagrees"
                .to_owned(),
            reviewed_payload: None,
            actor: actor.clone(),
        },
    )
    .await;
    if let Err(error) = decided {
        tracing::warn!(%error, proposal = %proposal.id, "automatic merge fell back to a suggestion");
        return Ok(AutonomyOutcome::LeftAsSuggestion);
    }
    let mut entry = NewActivity::new(
        proposal.project_id,
        "ai",
        proposal.model.clone(),
        actor,
        AutonomyTask::FuzzyDuplicates.as_str(),
        "duplicate_merged",
        format!(
            "The AI merged a duplicate record into an existing one: “{}”.",
            truncate(&title, 120)
        ),
    );
    entry.affected = json!([{"type": "record", "id": record_id, "label": title}]);
    entry.after_state = json!({"record_id": record_id});
    entry.undo_kind = Some("duplicate_link");
    entry.ai_run_id = Some(proposal.model_run_id);
    entry.proposal_id = Some(proposal.id);
    entry.model = Some(proposal.model.clone());
    entry.prompt_version = Some(proposal.prompt_version.clone());
    let mut tx = pool.begin().await?;
    let activity_id = record_activity_in_transaction(&mut tx, &entry).await?;
    tx.commit().await?;
    Ok(AutonomyOutcome::Applied { activity_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_near_identical_agreeing_matches_are_confident() {
        let confident = json!({
            "decision": "match",
            "signals": [{"kind": "title_similarity", "similarity": 0.98, "supports_match": true}]
        });
        assert!(duplicate_is_confident(&confident));
        let contradicted = json!({
            "decision": "match",
            "signals": [
                {"kind": "title_similarity", "similarity": 0.98, "supports_match": true},
                {"kind": "publication_year", "supports_match": false}
            ]
        });
        assert!(!duplicate_is_confident(&contradicted));
        let weak = json!({
            "decision": "match",
            "signals": [{"kind": "title_similarity", "similarity": 0.8, "supports_match": true}]
        });
        assert!(!duplicate_is_confident(&weak));
        assert!(!duplicate_is_confident(&json!({"decision": "no_match"})));
    }

    #[test]
    fn truncation_is_char_safe() {
        assert_eq!(truncate("héllo", 2), "hé…");
        assert_eq!(truncate("abc", 5), "abc");
    }
}
