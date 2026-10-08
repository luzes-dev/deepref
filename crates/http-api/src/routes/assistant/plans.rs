//! Assistant plans: what the assistant wants to change, shown to the user and
//! executed only after an explicit confirmation.

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use deepref_ai::{
    AgentTool, AgentToolName, PlanAction, TOOL_SCREEN_REPORTS, plan_action_is_executable,
};
use deepref_application::ScreenReportCommand;
use deepref_domain::{Actor, ProjectId, ScreeningDecision, ScreeningStage};
use serde::Serialize;
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{AssistantFailure, execute_proposal};
use crate::{
    error::{ApiError, ErrorResponse},
    routes::actor::extract_actor,
    state::AppState,
};

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AssistantPlanManualStepDto {
    pub reason: String,
    /// `protocol` or `full_text_screening`.
    pub link_target: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AssistantPlanActionDto {
    pub id: String,
    pub tool: String,
    pub summary: String,
    pub rationale: String,
    pub affected_count: u32,
    /// False when the assistant is never allowed to run this; see `manual`.
    pub executable: bool,
    pub manual: Option<AssistantPlanManualStepDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AssistantPlanResultDto {
    pub action_id: String,
    /// `executed`, `queued` (an AI review is running), `completed` (the review
    /// finished), `failed`, `skipped` or `manual`.
    pub status: String,
    pub message: String,
    pub review_run_id: Option<Uuid>,
    pub applied: Option<u32>,
    pub unchanged: Option<u32>,
    pub failed: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AssistantPlanDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub conversation_id: Uuid,
    /// `pending`, `confirmed`, `rejected`, `executed` or `failed`.
    pub status: String,
    pub summary: String,
    pub actions: Vec<AssistantPlanActionDto>,
    pub results: Option<Vec<AssistantPlanResultDto>>,
    pub error: Option<String>,
    pub model: String,
    pub prompt_version: String,
    pub created_at: DateTime<Utc>,
    pub resolved_by: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
}

fn parse_actions(value: &Value) -> Vec<PlanAction> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

fn result_dto(value: &Value) -> Option<AssistantPlanResultDto> {
    let field = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
    let count = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
    };
    Some(AssistantPlanResultDto {
        action_id: field("action_id")?,
        status: field("status")?,
        message: field("message").unwrap_or_default(),
        review_run_id: field("review_run_id").and_then(|id| Uuid::parse_str(&id).ok()),
        applied: count("applied"),
        unchanged: count("unchanged"),
        failed: count("failed"),
    })
}

pub(super) fn plan_dto(record: &deepref_postgres::AssistantPlanRecord) -> AssistantPlanDto {
    AssistantPlanDto {
        id: record.id,
        project_id: record.project_id,
        conversation_id: record.conversation_id,
        status: record.status.clone(),
        summary: record.summary.clone(),
        actions: parse_actions(&record.actions)
            .into_iter()
            .map(|action| AssistantPlanActionDto {
                id: action.id,
                tool: action.tool,
                summary: action.summary,
                rationale: action.rationale,
                affected_count: action.affected_count,
                executable: action.executable,
                manual: action.manual.map(|manual| AssistantPlanManualStepDto {
                    reason: manual.reason,
                    link_target: manual.link_target,
                }),
            })
            .collect(),
        results: record
            .results
            .as_ref()
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(result_dto).collect()),
        error: record.error.clone(),
        model: record.model.clone(),
        prompt_version: record.prompt_version.clone(),
        created_at: record.created_at,
        resolved_by: record.resolved_by_id.clone(),
        resolved_at: record.resolved_at,
    }
}

fn map_plan_error(error: deepref_postgres::AssistantError) -> ApiError {
    match error {
        deepref_postgres::AssistantError::Database(error) => ApiError::Database(error),
        _ => ApiError::NotFound("assistant plan not found".to_owned()),
    }
}

fn not_pending() -> ApiError {
    ApiError::Conflict {
        code: "assistant_plan_not_pending".to_owned(),
        message: "This plan was already confirmed or cancelled.".to_owned(),
        details: Value::Null,
    }
}

async fn load_plan(
    state: &AppState,
    project_id: Uuid,
    plan_id: Uuid,
) -> Result<deepref_postgres::AssistantPlanRecord, ApiError> {
    deepref_postgres::get_assistant_plan(&state.pool, project_id, plan_id)
        .await
        .map_err(map_plan_error)?
        .ok_or_else(|| ApiError::NotFound("assistant plan not found".to_owned()))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/assistant/plans/{plan_id}",
    operation_id = "getAssistantPlan",
    tag = "assistant",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("plan_id" = Uuid, Path, description = "Plan identifier")
    ),
    responses(
        (status = 200, description = "The plan and, once resolved, its results", body = AssistantPlanDto),
        (status = 404, description = "Plan not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_plan(
    State(state): State<AppState>,
    Path((project_id, plan_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AssistantPlanDto>, ApiError> {
    let record = load_plan(&state, project_id, plan_id).await?;
    let record = refresh_review_steps(&state, record).await;
    Ok(Json(plan_dto(&record)))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/assistant/plans/{plan_id}/reject",
    operation_id = "rejectAssistantPlan",
    tag = "assistant",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("plan_id" = Uuid, Path, description = "Plan identifier")
    ),
    responses(
        (status = 200, description = "The cancelled plan; nothing was changed", body = AssistantPlanDto),
        (status = 404, description = "Plan not found", body = ErrorResponse),
        (status = 409, description = "Plan is no longer pending", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn reject_plan(
    State(state): State<AppState>,
    Path((project_id, plan_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<AssistantPlanDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    load_plan(&state, project_id, plan_id).await?;
    let record = deepref_postgres::claim_assistant_plan(
        &state.pool,
        project_id,
        plan_id,
        "rejected",
        actor.kind().as_str(),
        actor.id(),
    )
    .await
    .map_err(map_plan_error)?
    .ok_or_else(not_pending)?;
    Ok(Json(plan_dto(&record)))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/assistant/plans/{plan_id}/confirm",
    operation_id = "confirmAssistantPlan",
    tag = "assistant",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("plan_id" = Uuid, Path, description = "Plan identifier")
    ),
    responses(
        (status = 200, description = "The executed plan with a result per action", body = AssistantPlanDto),
        (status = 404, description = "Plan not found", body = ErrorResponse),
        (status = 409, description = "Plan is no longer pending", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn confirm_plan(
    State(state): State<AppState>,
    Path((project_id, plan_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<AssistantPlanDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    load_plan(&state, project_id, plan_id).await?;
    // The pending -> confirmed transition is the exactly-once guard.
    let claimed = deepref_postgres::claim_assistant_plan(
        &state.pool,
        project_id,
        plan_id,
        "confirmed",
        actor.kind().as_str(),
        actor.id(),
    )
    .await
    .map_err(map_plan_error)?
    .ok_or_else(not_pending)?;

    let provenance = PlanProvenance::of(&claimed);
    let actions = parse_actions(&claimed.actions);
    let mut results = Vec::with_capacity(actions.len());
    let mut any_failed = false;
    for action in &actions {
        let result = run_action(
            &state,
            ProjectId::new(project_id),
            &provenance,
            &actor,
            action,
        )
        .await;
        any_failed |= result.get("status").and_then(Value::as_str) == Some("failed");
        results.push(result);
    }
    let any_queued = results
        .iter()
        .any(|result| result.get("status").and_then(Value::as_str) == Some("queued"));
    let finished = deepref_postgres::finish_assistant_plan(
        &state.pool,
        plan_id,
        if any_failed { "failed" } else { "executed" },
        &Value::Array(results),
        any_failed.then_some("one or more actions failed"),
    )
    .await
    .map_err(map_plan_error)?;

    let outcome_text = if any_failed {
        "I ran the plan, but some actions failed. Details are on the plan."
    } else if any_queued {
        "I queued the plan you confirmed. The AI review runs in the background, and the plan shows its result when it finishes."
    } else {
        "Done. I carried out the plan you confirmed."
    };
    if let Err(error) = deepref_postgres::append_assistant_message(
        &state.pool,
        &deepref_postgres::AppendAssistantMessage {
            id: None,
            conversation_id: finished.conversation_id,
            role: "assistant".to_owned(),
            content: outcome_text.to_owned(),
            tool_calls: None,
            tool_results: None,
            metadata: Some(json!({"plan_id": plan_id, "plan_result": true})),
        },
    )
    .await
    {
        tracing::warn!(%error, "failed to record plan outcome in the conversation");
    }
    Ok(Json(plan_dto(&finished)))
}

fn outcome(action: &PlanAction, status: &str, message: impl Into<String>) -> Value {
    json!({"action_id": action.id, "status": status, "message": message.into()})
}

fn failure_message(error: &AssistantFailure) -> &'static str {
    match error {
        AssistantFailure::NotFound => "The item no longer exists in this project.",
        AssistantFailure::Conflict => "This conflicts with the current state of the project.",
        AssistantFailure::Internal => "Something went wrong while doing this.",
    }
}

const QUEUED_MESSAGE: &str = "Queued for review. The AI review runs in the background; open the review queue to accept or reject its result.";
const REVIEW_COMPLETED_MESSAGE: &str =
    "The AI review finished. Open the review queue to accept or reject its proposal.";
const REVIEW_BLOCKED_MESSAGE: &str = "The AI review could not start because something it needs is missing, so no proposal was added. Open the review queue for details.";

/// Plain-language reason for a review that failed. The internal validation
/// detail is never shown to the user.
fn review_failure_message(detail: &str) -> &'static str {
    let lower = detail.to_lowercase();
    if lower.contains("budget") {
        "The AI budget for this month is used up, so the review did not run. No proposal was added."
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "The AI review took too long and was stopped. No proposal was added. Try again later."
    } else if lower.contains("validation") {
        "The AI answer did not pass our checks, so no proposal was added. Try again, or review this record yourself."
    } else {
        "The AI review failed, so no proposal was added. Try again later, or review this record yourself."
    }
}

/// What the assistant relied on when it made a plan. Copied onto the activity
/// rows that a confirmed plan creates, so each change can say why it happened.
#[derive(Debug, Clone)]
struct PlanProvenance {
    plan_id: Uuid,
    model: String,
    prompt_version: String,
    reads: Vec<String>,
}

impl PlanProvenance {
    fn of(record: &deepref_postgres::AssistantPlanRecord) -> Self {
        let reads: std::collections::BTreeSet<String> = record
            .evidence
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("tool").and_then(Value::as_str).map(str::to_owned))
            .collect();
        Self {
            plan_id: record.id,
            model: record.model.clone(),
            prompt_version: record.prompt_version.clone(),
            reads: reads.into_iter().collect(),
        }
    }

    /// "Why and what it was based on": the rationale of one action and the
    /// kinds of project data the assistant read in the same turn.
    fn activity_evidence(&self, action: &PlanAction) -> Value {
        let mut items = Vec::new();
        let rationale = action.rationale.trim();
        if !rationale.is_empty() {
            items.push(json!({"label": "Reasoning", "quote": rationale}));
        }
        let labels: std::collections::BTreeSet<&str> =
            self.reads.iter().map(|tool| read_label(tool)).collect();
        items.extend(
            labels
                .into_iter()
                .map(|label| json!({"label": format!("Read: {label}"), "quote": ""})),
        );
        Value::Array(items)
    }
}

/// A plain-language name for a read the assistant made, shown in the activity feed.
fn read_label(tool: &str) -> &'static str {
    match tool {
        "get_project_protocol" => "protocol",
        "get_report" => "report details",
        "read_document_blocks" | "search_document" => "full-text passages",
        "search_project_reports" => "search results",
        "get_screening_state" => "screening decisions",
        "get_study" | "list_studies" => "study details",
        "get_appraisal" => "appraisal answers",
        "get_project_overview" => "project overview",
        "list_reports_by_screening_status" => "screening status",
        _ => "project data",
    }
}

/// The autonomy task an AI review belongs to, so the activity feed files the
/// assistant's change under the same task as the review it started.
fn activity_task(action: &PlanAction) -> &'static str {
    match action.tool.as_str() {
        TOOL_SCREEN_REPORTS => "title_abstract_screening",
        "propose_screening_decision" => match action.args.get("stage").and_then(Value::as_str) {
            Some("full_text") => "full_text_screening",
            _ => "title_abstract_screening",
        },
        "propose_duplicate_merge" => "fuzzy_duplicates",
        "propose_study_grouping" => "study_grouping",
        "propose_classification" => "study_classification",
        "propose_extraction" => "extraction",
        "propose_appraisal_answer" => "appraisal",
        _ => "assistant_plan",
    }
}

/// One sentence, ending with exactly one full stop.
fn sentence(text: &str) -> String {
    format!(
        "{}.",
        text.trim().trim_end_matches(['.', '!', '?']).trim_end()
    )
}

/// Moves queued review steps on once their review run has finished, so a plan
/// shows whether the review produced a proposal rather than only that it was
/// scheduled. The new state is stored, so it stays once it is known.
async fn refresh_review_steps(
    state: &AppState,
    record: deepref_postgres::AssistantPlanRecord,
) -> deepref_postgres::AssistantPlanRecord {
    let stored = record.results.as_ref().and_then(Value::as_array).cloned();
    let Some(mut results) = stored else {
        return record;
    };
    let queued: Vec<Uuid> = results
        .iter()
        .filter(|result| result.get("status").and_then(Value::as_str) == Some("queued"))
        .filter_map(|result| result.get("review_run_id").and_then(Value::as_str))
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect();
    if queued.is_empty() {
        return record;
    }
    let Ok(runs) =
        deepref_postgres::get_review_runs(&state.pool, ProjectId::new(record.project_id), &queued)
            .await
    else {
        return record;
    };
    let mut changed = false;
    for result in &mut results {
        if result.get("status").and_then(Value::as_str) != Some("queued") {
            continue;
        }
        let Some(run_id) = result
            .get("review_run_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
        else {
            continue;
        };
        let Some(run) = runs.iter().find(|run| run.id.as_uuid() == run_id) else {
            continue;
        };
        let (status, message) = match &run.state {
            deepref_review::ReviewRunState::Completed { .. } => {
                ("completed", REVIEW_COMPLETED_MESSAGE)
            }
            deepref_review::ReviewRunState::Failed { message, .. } => {
                ("failed", review_failure_message(message))
            }
            deepref_review::ReviewRunState::Blocked { .. } => ("failed", REVIEW_BLOCKED_MESSAGE),
            deepref_review::ReviewRunState::Queued | deepref_review::ReviewRunState::Running => {
                continue;
            }
        };
        result["status"] = json!(status);
        result["message"] = json!(message);
        changed = true;
    }
    if !changed {
        return record;
    }
    let any_failed = results
        .iter()
        .any(|result| result.get("status").and_then(Value::as_str) == Some("failed"));
    let status = if any_failed { "failed" } else { "executed" };
    let error = any_failed.then_some("one or more AI reviews failed");
    let results = Value::Array(results);
    match deepref_postgres::update_assistant_plan_results(
        &state.pool,
        record.id,
        status,
        &results,
        error,
    )
    .await
    {
        Ok(updated) => updated,
        Err(error) => {
            tracing::warn!(%error, "failed to store refreshed assistant plan results");
            record
        }
    }
}

async fn run_action(
    state: &AppState,
    project_id: ProjectId,
    provenance: &PlanProvenance,
    actor: &Actor,
    action: &PlanAction,
) -> Value {
    if action.manual.is_some() {
        return outcome(action, "manual", "You need to do this yourself.");
    }
    // Policy is checked again at execution time, not only when planned.
    if !plan_action_is_executable(project_id, actor, action) {
        return outcome(
            action,
            "skipped",
            "This action is not allowed by the project policy.",
        );
    }
    if action.tool == TOOL_SCREEN_REPORTS {
        return run_screen_reports(state, project_id, provenance, actor, action).await;
    }
    let operation = AgentToolName::parse(&action.tool)
        .and_then(|name| AgentTool::from_name_and_args(name, action.args.clone()).ok())
        .and_then(|tool| tool.into_proposal_operation().ok());
    let Some(operation) = operation else {
        return outcome(action, "skipped", "The assistant cannot run this action.");
    };
    match execute_proposal(state, operation, actor.clone()).await {
        Ok(run) => {
            let mut entry = deepref_postgres::NewActivity::new(
                project_id.as_uuid(),
                "assistant",
                "Assistant",
                actor.clone(),
                activity_task(action),
                "ai_review_started",
                format!(
                    "The assistant started an AI review: {}",
                    sentence(&action.summary)
                ),
            );
            entry.batch_id = Some(provenance.plan_id);
            entry.model = Some(provenance.model.clone());
            entry.prompt_version = Some(provenance.prompt_version.clone());
            entry.evidence = provenance.activity_evidence(action);
            if let Err(error) = deepref_postgres::record_activity(&state.pool, &entry).await {
                tracing::warn!(%error, "failed to record assistant activity");
            }
            let mut value = outcome(action, "queued", QUEUED_MESSAGE);
            value["review_run_id"] = json!(run.id.as_uuid());
            value
        }
        Err(error) => outcome(action, "failed", failure_message(&error)),
    }
}

/// Every change made by a confirmed plan lands in the activity feed, grouped
/// by plan so the whole plan can be undone at once.
#[allow(clippy::too_many_arguments)]
async fn record_screening_activity(
    state: &AppState,
    project_id: ProjectId,
    provenance: &PlanProvenance,
    actor: &Actor,
    report_id: Uuid,
    decision: ScreeningDecision,
    event_id: Option<Uuid>,
    protocol_version_id: Uuid,
    evidence: &Value,
) {
    let Some(event_id) = event_id else {
        return;
    };
    let title = sqlx::query_scalar::<_, Option<String>>("SELECT title FROM reports WHERE id=$1")
        .bind(report_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .map(|title| deepref_postgres::decode_html_entities(&title))
        .unwrap_or_else(|| "Untitled record".to_owned());
    let verb = match decision {
        ScreeningDecision::Include => "included",
        ScreeningDecision::Exclude => "excluded",
        ScreeningDecision::Maybe => "marked as maybe",
    };
    let shown: String = title.chars().take(120).collect();
    let mut entry = deepref_postgres::NewActivity::new(
        project_id.as_uuid(),
        "assistant",
        "Assistant",
        actor.clone(),
        "title_abstract_screening",
        "screening_decision",
        format!("The assistant {verb} “{shown}” after you confirmed its plan."),
    );
    entry.affected = json!([{"type": "report", "id": report_id, "label": title}]);
    entry.after_state = json!({
        "report_id": report_id,
        "stage": "title_abstract",
        "event_id": event_id,
        "protocol_version_id": protocol_version_id,
    });
    entry.undo_kind = Some("screening_event");
    entry.batch_id = Some(provenance.plan_id);
    entry.model = Some(provenance.model.clone());
    entry.prompt_version = Some(provenance.prompt_version.clone());
    entry.evidence = evidence.clone();
    if let Err(error) = deepref_postgres::record_activity(&state.pool, &entry).await {
        tracing::warn!(%error, "failed to record assistant activity");
    }
}

async fn run_screen_reports(
    state: &AppState,
    project_id: ProjectId,
    provenance: &PlanProvenance,
    actor: &Actor,
    action: &PlanAction,
) -> Value {
    let decision = match action.args.get("decision").and_then(Value::as_str) {
        Some("include") => ScreeningDecision::Include,
        Some("exclude") => ScreeningDecision::Exclude,
        Some("maybe") => ScreeningDecision::Maybe,
        _ => return outcome(action, "skipped", "The decision is missing."),
    };
    let protocol =
        match deepref_postgres::get_published_protocol(&state.pool, project_id.as_uuid()).await {
            Ok(protocol) => protocol,
            Err(_) => {
                return outcome(
                    action,
                    "failed",
                    "There is no published protocol yet. Publish the protocol before screening.",
                );
            }
        };
    let user_note = action
        .args
        .get("notes")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|note| !note.is_empty());
    let attribution = format!("via assistant (plan {})", provenance.plan_id);
    let notes = Some(user_note.map_or(attribution.clone(), |note| {
        format!("{note} [{attribution}]")
    }));
    let evidence = provenance.activity_evidence(action);

    let report_ids: Vec<Uuid> = action
        .args
        .get("report_ids")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().and_then(|text| Uuid::parse_str(text).ok()))
                .collect()
        })
        .unwrap_or_default();
    let (mut applied, mut unchanged, mut failed) = (0_u32, 0_u32, 0_u32);
    for report_id in report_ids {
        let revision = sqlx::query_scalar::<_, i64>(
            "SELECT revision FROM screening_state WHERE project_id = $1 AND report_id = $2",
        )
        .bind(project_id.as_uuid())
        .bind(report_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
        let command = ScreenReportCommand {
            project_id,
            report_id: report_id.into(),
            stage: ScreeningStage::TitleAbstract,
            decision,
            exclusion_reason_id: None,
            protocol_version_id: protocol.id.into(),
            expected_revision: revision,
            notes: notes.clone(),
            actor: actor.clone(),
        };
        match deepref_postgres::screen_report(&state.pool, command).await {
            Ok(snapshot) => {
                applied += 1;
                record_screening_activity(
                    state,
                    project_id,
                    provenance,
                    actor,
                    report_id,
                    decision,
                    snapshot.last_event_id,
                    protocol.id,
                    &evidence,
                )
                .await;
            }
            Err(deepref_postgres::ScreeningError::Repeated { .. }) => unchanged += 1,
            Err(error) => {
                tracing::warn!(%error, %report_id, "assistant plan: screening decision failed");
                failed += 1;
            }
        }
    }
    let status = if failed > 0 && applied == 0 && unchanged == 0 {
        "failed"
    } else {
        "executed"
    };
    let message = format!(
        "{applied} decision(s) recorded, {unchanged} already that way, {failed} could not be recorded."
    );
    let mut value = outcome(action, status, message);
    value["applied"] = json!(applied);
    value["unchanged"] = json!(unchanged);
    value["failed"] = json!(failed);
    value
}

#[cfg(test)]
mod tests {
    use super::{read_label, review_failure_message, sentence};

    #[test]
    fn review_failures_are_explained_without_internal_detail() {
        let validation =
            review_failure_message("review execution failed: AI output failed semantic validation");
        assert!(validation.starts_with("The AI answer did not pass our checks"));
        assert!(!validation.contains("validation"));
        assert!(review_failure_message("monthly budget exhausted").starts_with("The AI budget"));
        assert!(review_failure_message("boom").starts_with("The AI review failed"));
    }

    #[test]
    fn activity_sentences_end_with_exactly_one_full_stop() {
        assert_eq!(
            sentence("Exclude the record as off-topic."),
            "Exclude the record as off-topic."
        );
        assert_eq!(
            sentence("Exclude the record as off-topic"),
            "Exclude the record as off-topic."
        );
        assert_eq!(read_label("search_document"), "full-text passages");
        assert_eq!(read_label("unknown_tool"), "project data");
    }
}
