//! Block executors. Every executor receives its inputs already resolved and
//! returns the outputs and the ports that carry data. In test mode blocks with
//! side effects report what they would have done and change nothing.

use chrono::Utc;
use deepref_ai::{
    AiGateway, CompletionRequest, ModelProfile, ModelRouter, ScreeningStage as AiStage,
};
use deepref_application::{
    ScreenReportCommand,
    workflows::{
        AutonomyLevel, AutonomyTask, Condition, ReviewSlot, Routed, SCREENING_BATCH,
        SCREENING_POLL_SECS, ScreeningWait, Verdict, evaluate_condition, extract_identifiers,
        gating::truncate, judge_records, notification_fingerprint, preflight, progress_note,
        render_template, report_ids_in, slack_webhook_problem, summary_note,
    },
};
use deepref_domain::{
    Actor, ActorKind, ExclusionReasonId, ProjectId, ProtocolVersionId, ReportId, ScreeningDecision,
    ScreeningStage,
};
use deepref_postgres::{
    AiProposalError, NodeExecution, NodeOutcome, PostgresAiStore, RecordFilter, get_ai_budget,
    get_ai_proposal, get_review_runs, get_workflow_secret,
};
use deepref_review::ReviewRunState;
use serde_json::{Map, Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{NodeError, autonomy_gate, net, pubmed};

pub struct Context<'a> {
    pub pool: &'a PgPool,
    pub ai: Option<&'a dyn AiGateway>,
    pub execution: &'a NodeExecution,
}

const MAX_BATCH: usize = 200;

type NodeResult = Result<NodeOutcome, NodeError>;

fn ok(port: &str, value: Value, note: Option<String>) -> NodeResult {
    let mut outputs = Map::new();
    outputs.insert(port.to_owned(), value);
    Ok(NodeOutcome::Completed {
        outputs,
        fired: vec![port.to_owned()],
        items: 0,
        note,
    })
}

fn skipped(note: &str) -> NodeResult {
    Ok(NodeOutcome::Skipped {
        note: note.to_owned(),
    })
}

fn items_of(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![other.clone()],
    }
}

fn report_ids(items: &[Value]) -> Vec<Uuid> {
    items
        .iter()
        .filter_map(|item| item.get("report_id").and_then(Value::as_str))
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect()
}

fn cfg_str<'a>(config: &'a Value, key: &str) -> Option<&'a str> {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn cfg_i64(config: &Value, key: &str, default: i64) -> i64 {
    config.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn project(context: &Context<'_>) -> ProjectId {
    ProjectId::new(context.execution.project_id)
}

fn automation_actor(context: &Context<'_>) -> Result<Actor, NodeError> {
    Actor::new(
        ActorKind::Automation,
        format!("workflow:{}", context.execution.workflow_id),
    )
    .map_err(|_| NodeError::permanent("The automation identity is not valid."))
}

async fn autonomy(context: &Context<'_>, task: AutonomyTask) -> Result<AutonomyLevel, NodeError> {
    autonomy_gate()
        .decide(task, project(context))
        .await
        .map_err(|error| NodeError::retryable(error.to_string()))
}

fn dry_note(what: &str) -> Option<String> {
    Some(format!("Test run: nothing was changed. {what}"))
}

pub async fn execute(context: &Context<'_>) -> NodeResult {
    let execution = context.execution;
    let config = &execution.config;
    let input = |port: &str| execution.inputs.get(port);
    match execution.node_type.as_str() {
        // ---- Data --------------------------------------------------------
        "data.find_records" => {
            let filter = RecordFilter {
                stage: cfg_str(config, "stage").unwrap_or("final").to_owned(),
                status: cfg_str(config, "status").unwrap_or("any").to_owned(),
                text: cfg_str(config, "text").map(str::to_owned),
                limit: cfg_i64(config, "limit", 100),
            };
            let items =
                deepref_postgres::find_record_items(context.pool, project(context), &filter)
                    .await?;
            let note = format!("Found {} record(s).", items.len());
            ok("records", Value::Array(items), Some(note))
        }
        "data.screening_queue" => {
            let items = deepref_postgres::screening_queue_items(
                context.pool,
                project(context),
                cfg_str(config, "stage").unwrap_or("title_abstract"),
                cfg_i64(config, "limit", 100),
            )
            .await?;
            let note = format!("{} record(s) waiting.", items.len());
            ok("records", Value::Array(items), Some(note))
        }
        "data.get_report" => {
            let ids = report_ids(&items_of(input("record")));
            let Some(id) = ids.first() else {
                return skipped("There was no record to look up.");
            };
            match deepref_postgres::report_details(context.pool, project(context), *id).await? {
                Some(details) => ok("report", details, None),
                None => skipped("The record is no longer in this project."),
            }
        }
        "data.get_study" => {
            let ids = report_ids(&items_of(input("record")));
            let Some(id) = ids.first() else {
                return skipped("There was no record to look up.");
            };
            match deepref_postgres::study_of_report(context.pool, project(context), *id).await? {
                Some(study) => ok("study", study, None),
                None => skipped("This record is not part of a study yet."),
            }
        }
        // ---- Logic -------------------------------------------------------
        "logic.if" => {
            let condition: Condition =
                serde_json::from_value(config.get("condition").cloned().unwrap_or(Value::Null))
                    .map_err(|_| NodeError::permanent("The rule of this step is incomplete."))?;
            let data = input("data").cloned().unwrap_or(Value::Null);
            let subject = match &data {
                Value::Array(list) => json!({ "count": list.len(), "items": list }),
                other => other.clone(),
            };
            let port = if evaluate_condition(&condition, &subject) {
                "yes"
            } else {
                "no"
            };
            ok(port, data, Some(format!("The answer was \"{port}\".")))
        }
        "logic.filter" => {
            let condition: Condition =
                serde_json::from_value(config.get("condition").cloned().unwrap_or(Value::Null))
                    .map_err(|_| NodeError::permanent("The rule of this step is incomplete."))?;
            let (kept, rejected): (Vec<Value>, Vec<Value>) = items_of(input("records"))
                .into_iter()
                .partition(|item| evaluate_condition(&condition, item));
            let stop = config
                .get("stop_if_empty")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if kept.is_empty() && stop {
                return skipped("No record matched the rule, so this branch stopped.");
            }
            let note = format!("Kept {}, set aside {}.", kept.len(), rejected.len());
            let mut outputs = Map::new();
            outputs.insert("records".to_owned(), Value::Array(kept));
            outputs.insert("rejected".to_owned(), Value::Array(rejected));
            Ok(NodeOutcome::Completed {
                outputs,
                fired: vec!["records".to_owned(), "rejected".to_owned()],
                items: 0,
                note: Some(note),
            })
        }
        "logic.for_each" => {
            let max = usize::try_from(cfg_i64(config, "max_items", 50).clamp(1, 200)).unwrap_or(50);
            let mut items = items_of(input("items"));
            let total = items.len();
            items.truncate(max);
            if items.is_empty() {
                return skipped("There was nothing to repeat for.");
            }
            let count = items.len();
            let mut outputs = Map::new();
            outputs.insert("item".to_owned(), Value::Array(items));
            Ok(NodeOutcome::Completed {
                outputs,
                fired: vec!["item".to_owned()],
                items: count,
                note: Some(if total > count {
                    format!("Repeating for the first {count} of {total} records.")
                } else {
                    format!("Repeating for {count} record(s).")
                }),
            })
        }
        // The delay is applied when the step is queued.
        "logic.wait" => ok("data", input("data").cloned().unwrap_or(Value::Null), None),
        "logic.merge" => ok(
            "merged",
            input("inputs").cloned().unwrap_or(Value::Array(Vec::new())),
            None,
        ),
        // ---- Integrations -------------------------------------------------
        "integration.notify" => {
            let data = input("data").cloned().unwrap_or(Value::Null);
            let title = render_template(cfg_str(config, "title").unwrap_or("Automation"), &data);
            let message = render_template(cfg_str(config, "message").unwrap_or_default(), &data);
            if execution.test_mode {
                return ok(
                    "data",
                    data,
                    dry_note(&format!("Would have sent the notification \"{title}\".")),
                );
            }
            // The same notification about the same records is not added twice:
            // a manual run repeated over the same records adds nothing new.
            let fingerprint = notification_fingerprint(
                execution.workflow_id,
                &report_ids_in(&data),
                &title,
                &message,
            );
            let sent = deepref_postgres::notify_from_workflow(
                context.pool,
                execution.project_id,
                cfg_str(config, "severity").unwrap_or("info"),
                if title.trim().is_empty() {
                    "Automation"
                } else {
                    &title
                },
                Some(&message),
                json!({ "workflow_id": execution.workflow_id, "run_id": execution.run_id }),
                fingerprint.as_deref(),
            )
            .await?;
            if sent {
                ok("data", data, Some("Notification sent.".to_owned()))
            } else {
                ok(
                    "data",
                    data,
                    Some(
                        "Not sent again: the same notification about these records is already in your notifications."
                            .to_owned(),
                    ),
                )
            }
        }
        "integration.email" => {
            let data = input("data").cloned().unwrap_or(Value::Null);
            let recipients: Vec<String> =
                render_template(cfg_str(config, "to").unwrap_or_default(), &data)
                    .split(',')
                    .map(|address| address.trim().to_owned())
                    .filter(|address| !address.is_empty())
                    .collect();
            let subject = render_template(cfg_str(config, "subject").unwrap_or_default(), &data);
            let body = render_template(cfg_str(config, "body").unwrap_or_default(), &data);
            // The checks a real send makes first, so a test fails where a real run would.
            net::email_preflight(&recipients)?;
            if execution.test_mode {
                return ok(
                    "data",
                    data,
                    dry_note(&format!("Would have e-mailed {}.", recipients.join(", "))),
                );
            }
            net::send_email(&recipients, &subject, &body).await?;
            ok("data", data, Some("E-mail sent.".to_owned()))
        }
        "integration.slack" => {
            let data = input("data").cloned().unwrap_or(Value::Null);
            let message = render_template(cfg_str(config, "message").unwrap_or_default(), &data);
            let webhook = get_workflow_secret(
                context.pool,
                execution.workflow_id,
                &execution.node_id,
                "webhook",
            )
            .await?
            .ok_or_else(|| NodeError::permanent(preflight::SLACK_WEBHOOK_MISSING))?;
            // Checked before a test run reports anything, so it fails where a real run would.
            if let Some(problem) = slack_webhook_problem(&webhook) {
                return Err(NodeError::permanent(problem));
            }
            if execution.test_mode {
                return ok(
                    "data",
                    data,
                    dry_note("Would have posted a message to Slack."),
                );
            }
            let response = net::send(
                "POST",
                &webhook,
                &[("Content-Type".to_owned(), "application/json".to_owned())],
                Some(json!({ "text": message }).to_string()),
            )
            .await?;
            if response.status >= 500 {
                return Err(NodeError::retryable("Slack is not answering right now."));
            }
            if response.status >= 300 {
                return Err(NodeError::permanent(
                    "Slack refused the message. Check the webhook address.",
                ));
            }
            ok("data", data, Some("Posted to Slack.".to_owned()))
        }
        "integration.http_request" => {
            let data = input("data").cloned().unwrap_or(Value::Null);
            let method = cfg_str(config, "method").unwrap_or("POST").to_uppercase();
            let url = render_template(cfg_str(config, "url").unwrap_or_default(), &data);
            let body = cfg_str(config, "body").map(|text| render_template(text, &data));
            if execution.test_mode {
                // Resolved the way a real call is, so a test fails where a real run would.
                // Nothing is sent.
                net::resolve_public(&url).await?;
                return ok(
                    "response",
                    json!({ "dry_run": true, "method": method, "url": url }),
                    dry_note(&format!("Would have called {url}.")),
                );
            }
            let mut headers: Vec<(String, String)> = cfg_str(config, "headers")
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| (name.trim().to_owned(), render_template(value.trim(), &data)))
                .filter(|(name, _)| !name.is_empty())
                .collect();
            if let Some(auth) = get_workflow_secret(
                context.pool,
                execution.workflow_id,
                &execution.node_id,
                "auth",
            )
            .await?
            {
                headers.push(("Authorization".to_owned(), auth));
            }
            if body.is_some()
                && !headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            {
                headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
            }
            let response = net::send(&method, &url, &headers, body).await?;
            if response.status >= 500 {
                return Err(NodeError::retryable(format!(
                    "The other service had a problem ({}).",
                    response.status
                )));
            }
            let parsed = serde_json::from_str::<Value>(&response.body)
                .unwrap_or(Value::String(response.body));
            if response.status >= 400 {
                return Err(NodeError::permanent(format!(
                    "The other service refused the request ({}).",
                    response.status
                )));
            }
            ok(
                "response",
                json!({ "status": response.status, "body": parsed }),
                None,
            )
        }
        // ---- Actions -------------------------------------------------------
        "action.recompute_metrics" => {
            if execution.test_mode {
                return ok(
                    "summary",
                    json!({ "dry_run": true }),
                    dry_note("Would have refreshed the project statistics."),
                );
            }
            deepref_postgres::recompute_project_metrics(context.pool, execution.project_id)
                .await
                .map_err(|_| {
                    NodeError::retryable(
                        "The statistics could not be refreshed. This will be tried again.",
                    )
                })?;
            ok("summary", json!({ "refreshed": true }), None)
        }
        "action.run_deduplication" => run_deduplication(context).await,
        "action.import_identifiers" => import_identifiers(context).await,
        "action.record_decision" => record_decision(context).await,
        "action.group_into_study" => group_into_study(context).await,
        "action.attach_pdf" => attach_pdf(context).await,
        "action.export" => export_records(context).await,
        // ---- AI --------------------------------------------------------------
        "ai.prompt" => ai_prompt(context).await,
        "ai.classify" => {
            let stage = cfg_str(config, "stage").unwrap_or("title_abstract");
            schedule_reviews(context, "screening", stage).await
        }
        "ai.extract_field" => schedule_reviews(context, "data_extraction", "title_abstract").await,
        "ai.run_review" => {
            let task = cfg_str(config, "task").unwrap_or("screening");
            let stage = cfg_str(config, "stage").unwrap_or("title_abstract");
            if task == "screening" {
                return screening_review(context, stage).await;
            }
            schedule_reviews(context, task, stage).await
        }
        other => Err(NodeError::permanent(format!(
            "The block \"{other}\" is not available."
        ))),
    }
}

// ---------------------------------------------------------------------------

async fn run_deduplication(context: &Context<'_>) -> NodeResult {
    let execution = context.execution;
    match autonomy(context, AutonomyTask::ExactDuplicates).await? {
        AutonomyLevel::Off => {
            return skipped("Duplicate checking is switched off for this project.");
        }
        _ if execution.test_mode => {
            return ok(
                "summary",
                json!({ "dry_run": true }),
                dry_note("Would have checked for duplicates."),
            );
        }
        _ => {}
    }
    let summary = deepref_postgres::run_deduplication(
        context.pool,
        deepref_postgres::DedupeRunRequest {
            project_id: execution.project_id,
            limit: cfg_i64(&execution.config, "limit", 500).clamp(1, 5000),
            actor_kind: ActorKind::Automation.as_str().to_owned(),
            actor_id: format!("workflow:{}", execution.workflow_id),
        },
    )
    .await
    .map_err(|_| {
        NodeError::retryable("The duplicate check could not finish. This will be tried again.")
    })?;
    ok(
        "summary",
        json!({
            "checked": summary.processed,
            "linked": summary.auto_linked,
            "new_reports": summary.created_reports,
            "suggestions": summary.proposals_created,
        }),
        Some(format!("Checked {} record(s).", summary.processed)),
    )
}

fn collect_identifiers(value: &Value, dois: &mut Vec<String>, pmids: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            let found = extract_identifiers(text);
            for doi in found.dois {
                if !dois.contains(&doi) {
                    dois.push(doi);
                }
            }
            for pmid in found.pmids {
                if !pmids.contains(&pmid) {
                    pmids.push(pmid);
                }
            }
        }
        Value::Array(list) => list
            .iter()
            .for_each(|item| collect_identifiers(item, dois, pmids)),
        Value::Object(map) => {
            for (key, item) in map {
                if key == "pmid" {
                    if let Some(id) = item
                        .as_str()
                        .or(None)
                        .map(str::to_owned)
                        .or_else(|| item.as_u64().map(|n| n.to_string()))
                        && !pmids.contains(&id)
                    {
                        pmids.push(id);
                    }
                } else if key == "html" {
                    continue;
                } else {
                    collect_identifiers(item, dois, pmids);
                }
            }
        }
        _ => {}
    }
}

async fn import_identifiers(context: &Context<'_>) -> NodeResult {
    let execution = context.execution;
    let mut dois = Vec::new();
    let mut pmids = Vec::new();
    if let Some(items) = execution.inputs.get("items") {
        collect_identifiers(items, &mut dois, &mut pmids);
    }
    dois.truncate(MAX_BATCH);
    pmids.truncate(MAX_BATCH);
    if dois.is_empty() && pmids.is_empty() {
        return skipped("No DOI or PubMed ID was found.");
    }
    if execution.test_mode {
        return ok(
            "records",
            json!([]),
            dry_note(&format!(
                "Would have added {} DOI(s) and {} PubMed ID(s).",
                dois.len(),
                pmids.len()
            )),
        );
    }
    let settings = crate::store::load_runtime_settings(context.pool)
        .await
        .map_err(|_| NodeError::retryable("The settings could not be read."))?;
    // What the project already holds under these identifiers, before anything is added.
    let before = identifier_reports(context, &dois, &pmids).await?;
    let mut records = Vec::new();
    let mut unresolved = 0usize;
    if !dois.is_empty() {
        let provider = deepref_providers::CrossrefProvider::new(settings.crossref_mailto.clone())
            .map_err(|_| {
            NodeError::permanent("The Crossref connection could not be prepared.")
        })?;
        for doi in &dois {
            match provider.fetch_work_with_references(doi).await {
                Ok(work) => records.push(deepref_providers::raw_record_from_crossref_work(work)),
                Err(_) => unresolved += 1,
            }
        }
    }
    if !pmids.is_empty() {
        let client = deepref_providers::PubmedClient::new(
            net::trusted_client()?,
            settings.crossref_mailto.clone(),
        );
        let articles = pubmed::fetch_pubmed(context.pool, &client, &pmids).await?;
        records.extend(
            articles
                .iter()
                .map(deepref_providers::PubmedArticle::to_raw_record),
        );
    }
    if records.is_empty() {
        return Err(NodeError::permanent(
            "None of the identifiers could be found.",
        ));
    }
    let label = cfg_str(&execution.config, "label")
        .unwrap_or("Added by automation")
        .to_owned();
    deepref_postgres::persist_import(
        context.pool,
        &deepref_postgres::ImportPersistRequest {
            project_id: execution.project_id,
            source: "workflow".to_owned(),
            strategy: "workflow_import".to_owned(),
            format: deepref_domain::ImportFormat::Doi,
            idempotency_key: Some(format!(
                "workflow:{}:{}:{}",
                execution.run_id, execution.node_id, execution.iteration
            )),
            config: json!({ "label": label, "dois": dois, "pmids": pmids }),
            metadata: json!({ "workflow_id": execution.workflow_id }),
        },
        &records,
    )
    .await
    .map_err(|_| {
        NodeError::retryable("The records could not be saved. This will be tried again.")
    })?;
    // Resolve the new records into reports so later steps can use them.
    deepref_postgres::run_deduplication(
        context.pool,
        deepref_postgres::DedupeRunRequest {
            project_id: execution.project_id,
            limit: 1000,
            actor_kind: ActorKind::Automation.as_str().to_owned(),
            actor_id: format!("workflow:{}", execution.workflow_id),
        },
    )
    .await
    .map_err(|_| NodeError::retryable("The new records could not be checked for duplicates."))?;
    let ids = identifier_reports(context, &dois, &pmids).await?;
    let items = deepref_postgres::load_record_items(context.pool, project(context), &ids).await?;
    // The import saves a copy of each record even when the project already holds it;
    // the copy is then merged. Counting reports, not copies, says what was new.
    let (added, already) = import_counts(&before, &ids);
    ok(
        "records",
        Value::Array(items),
        Some(import_note(added, already, unresolved)),
    )
}

/// Reports in the project that carry one of these identifiers, sorted and unique.
async fn identifier_reports(
    context: &Context<'_>,
    dois: &[String],
    pmids: &[String],
) -> Result<Vec<Uuid>, NodeError> {
    let mut ids =
        deepref_postgres::find_reports_by_identifier(context.pool, project(context), "doi", dois)
            .await?;
    ids.extend(
        deepref_postgres::find_reports_by_identifier(context.pool, project(context), "pmid", pmids)
            .await?,
    );
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// Split the reports an import resolved to into those it added and those that
/// were already in the project. Returns `(added, already)`.
fn import_counts(before: &[Uuid], after: &[Uuid]) -> (usize, usize) {
    let already = after.iter().filter(|id| before.contains(id)).count();
    (after.len() - already, already)
}

/// The note a DOI or PubMed import shows, worded so a repeat is not reported as new.
fn import_note(added: usize, already: usize, unresolved: usize) -> String {
    let mut note = match (added, already) {
        (0, 0) => "No new record was added.".to_owned(),
        (added, 0) => format!("Added {added} record(s)."),
        (0, already) => {
            format!("{already} record(s) already in the project; nothing new was added.")
        }
        (added, already) => {
            format!("Added {added} record(s); {already} already in the project.")
        }
    };
    if unresolved > 0 {
        note.push_str(&format!(" {unresolved} DOI(s) could not be found."));
    }
    note
}

async fn current_revision(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<i64, NodeError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT revision FROM screening_state WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(0))
}

async fn record_decision(context: &Context<'_>) -> NodeResult {
    let execution = context.execution;
    let config = &execution.config;
    let stage = cfg_str(config, "stage").unwrap_or("title_abstract");
    let decision = cfg_str(config, "decision").unwrap_or("include");
    let items: Vec<Value> = items_of(execution.inputs.get("records"))
        .into_iter()
        .take(MAX_BATCH)
        .collect();
    if items.is_empty() {
        return skipped("There were no records to decide on.");
    }
    let task = if stage == "full_text" {
        AutonomyTask::FullTextScreening
    } else {
        AutonomyTask::TitleAbstractScreening
    };
    let level = autonomy(context, task).await?;
    if level == AutonomyLevel::Off {
        return skipped("Automatic screening decisions are switched off for this project.");
    }
    if execution.test_mode {
        return ok(
            "records",
            Value::Array(items.clone()),
            dry_note(&format!(
                "Would have marked {} record(s) as \"{decision}\".",
                items.len()
            )),
        );
    }
    if level == AutonomyLevel::SecondReviewer {
        // The automation's decision is an independent second opinion; a
        // person still makes the actual screening decision.
        let actor = automation_actor(context)?;
        let mut recorded = 0usize;
        for report_id in report_ids(&items) {
            if deepref_postgres::record_workflow_reviewer_decision(
                context.pool,
                execution.project_id,
                report_id,
                stage,
                decision,
                &actor,
                cfg_str(config, "note"),
                Some(execution.run_id),
            )
            .await
            .is_ok()
            {
                recorded += 1;
            }
        }
        return ok(
            "records",
            Value::Array(items.clone()),
            Some(format!(
                "Recorded \"{decision}\" as a second opinion for {recorded} record(s). A person still decides."
            )),
        );
    }
    if level != AutonomyLevel::Act {
        notify_suggestion(
            context,
            "Suggested screening decisions",
            &format!(
                "An automation suggests marking {} record(s) as \"{decision}\" ({stage}). Review them in the screening queue.",
                items.len()
            ),
            json!({ "stage": stage, "decision": decision, "report_ids": report_ids(&items) }),
        )
        .await?;
        return ok(
            "records",
            Value::Array(items.clone()),
            Some(format!(
                "Suggested \"{decision}\" for {} record(s); a person must confirm. Nothing was changed.",
                items.len()
            )),
        );
    }
    let project_id = project(context);
    let protocol = deepref_postgres::get_published_protocol(context.pool, execution.project_id)
        .await
        .map_err(|_| {
            NodeError::permanent("Publish the protocol before screening automatically.")
        })?;
    let domain_stage = if stage == "full_text" {
        ScreeningStage::FullText
    } else {
        ScreeningStage::TitleAbstract
    };
    let domain_decision = match decision {
        "exclude" => ScreeningDecision::Exclude,
        "maybe" => ScreeningDecision::Maybe,
        _ => ScreeningDecision::Include,
    };
    let reason = match cfg_str(config, "reason") {
        Some(name) => {
            deepref_postgres::find_exclusion_reason(context.pool, project_id, stage, name)
                .await?
                .map(ExclusionReasonId::new)
        }
        None => None,
    };
    if domain_decision == ScreeningDecision::Exclude && stage == "full_text" && reason.is_none() {
        return Err(NodeError::permanent(
            "Excluding at full text needs a reason that exists in the protocol.",
        ));
    }
    let actor = automation_actor(context)?;
    let (mut done, mut unchanged, mut failed) = (0usize, 0usize, 0usize);
    let mut decided = Vec::new();
    for item in &items {
        let Some(report_id) = report_ids(std::slice::from_ref(item)).first().copied() else {
            continue;
        };
        let revision = current_revision(context.pool, execution.project_id, report_id).await?;
        let command = ScreenReportCommand {
            project_id,
            report_id: ReportId::new(report_id),
            stage: domain_stage,
            decision: domain_decision,
            exclusion_reason_id: if domain_decision == ScreeningDecision::Exclude
                && stage == "full_text"
            {
                reason
            } else {
                None
            },
            protocol_version_id: ProtocolVersionId::new(protocol.id),
            expected_revision: revision,
            notes: cfg_str(config, "note").map(str::to_owned),
            actor: actor.clone(),
        };
        match deepref_postgres::screen_report(context.pool, command).await {
            Ok(snapshot) => {
                done += 1;
                decided.push(item.clone());
                if let Some(event_id) = snapshot.last_event_id {
                    let title = item
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Untitled record")
                        .to_owned();
                    let shown: String = title.chars().take(120).collect();
                    let mut entry = deepref_postgres::NewActivity::new(
                        execution.project_id,
                        "automation",
                        "Automation",
                        actor.clone(),
                        task.as_str(),
                        "screening_decision",
                        format!("An automation marked “{shown}” as \"{decision}\"."),
                    );
                    entry.affected = json!([{"type": "report", "id": report_id, "label": title}]);
                    entry.after_state = json!({
                        "report_id": report_id,
                        "stage": stage,
                        "event_id": event_id,
                        "protocol_version_id": protocol.id,
                    });
                    entry.undo_kind = Some("screening_event");
                    entry.batch_id = Some(execution.run_id);
                    let _ = deepref_postgres::record_activity(context.pool, &entry).await;
                }
            }
            Err(deepref_postgres::ScreeningError::Repeated { .. }) => unchanged += 1,
            Err(_) => failed += 1,
        }
    }
    ok(
        "records",
        Value::Array(decided),
        Some(format!(
            "Decided {done}; {unchanged} already had that decision; {failed} could not be changed."
        )),
    )
}

async fn notify_suggestion(
    context: &Context<'_>,
    title: &str,
    body: &str,
    payload: Value,
) -> Result<(), NodeError> {
    let mut payload = payload;
    if let Value::Object(map) = &mut payload {
        map.insert(
            "workflow_id".to_owned(),
            json!(context.execution.workflow_id),
        );
        map.insert("run_id".to_owned(), json!(context.execution.run_id));
    }
    deepref_postgres::notify_from_workflow(
        context.pool,
        context.execution.project_id,
        "info",
        title,
        Some(body),
        payload,
        None,
    )
    .await?;
    Ok(())
}

async fn group_into_study(context: &Context<'_>) -> NodeResult {
    let execution = context.execution;
    let items = items_of(execution.inputs.get("records"));
    if items.is_empty() {
        return skipped("There were no records to group.");
    }
    if autonomy(context, AutonomyTask::StudyGrouping).await? == AutonomyLevel::Off {
        return skipped("Grouping into studies is switched off for this project.");
    }
    let title = cfg_str(&execution.config, "title")
        .map(|title| render_template(title, items.first().unwrap_or(&Value::Null)))
        .or_else(|| {
            items
                .first()
                .and_then(|item| item.get("title"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "New study".to_owned());
    if execution.test_mode {
        return ok(
            "study",
            json!({ "title": title, "dry_run": true }),
            dry_note(&format!(
                "Would have grouped {} record(s) into \"{title}\".",
                items.len()
            )),
        );
    }
    // Creating studies on its own needs the autonomy setting to be wired to
    // the audit trail; until then the automation only suggests.
    notify_suggestion(
        context,
        "Suggested study group",
        &format!(
            "An automation suggests grouping {} record(s) into the study \"{title}\".",
            items.len()
        ),
        json!({ "title": title, "report_ids": report_ids(&items) }),
    )
    .await?;
    ok(
        "study",
        json!({ "title": title, "suggested": true }),
        Some("Suggested the study group; a person must confirm. Nothing was changed.".to_owned()),
    )
}

async fn attach_pdf(context: &Context<'_>) -> NodeResult {
    let execution = context.execution;
    let items: Vec<Value> = items_of(execution.inputs.get("records"))
        .into_iter()
        .take(MAX_BATCH)
        .collect();
    if items.is_empty() {
        return skipped("There were no records to attach a full text to.");
    }
    let source = cfg_str(&execution.config, "source").unwrap_or("open_access");
    if execution.test_mode {
        return ok(
            "records",
            Value::Array(items.clone()),
            dry_note(&format!(
                "Would have looked for the full text of {} record(s).",
                items.len()
            )),
        );
    }
    let settings = crate::store::load_runtime_settings(context.pool)
        .await
        .map_err(|_| NodeError::retryable("The settings could not be read."))?;
    let client = net::trusted_client()?;
    let actor = automation_actor(context)?;
    let (mut attached, mut missing) = (0usize, 0usize);
    for item in &items {
        let Some(report_id) = report_ids(std::slice::from_ref(item)).first().copied() else {
            continue;
        };
        let url = if source == "url" {
            cfg_str(&execution.config, "url").map(|template| render_template(template, item))
        } else {
            match item.get("doi").and_then(Value::as_str) {
                Some(doi) if !settings.crossref_mailto.trim().is_empty() => {
                    let lookup = format!(
                        "https://api.unpaywall.org/v2/{doi}?email={}",
                        settings.crossref_mailto.trim()
                    );
                    match net::trusted_get(&client, &lookup).await {
                        Ok(body) => serde_json::from_str::<Value>(&body).ok().and_then(|value| {
                            value
                                .pointer("/best_oa_location/url_for_pdf")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        }),
                        Err(_) => None,
                    }
                }
                _ => None,
            }
        };
        let Some(url) = url else {
            missing += 1;
            continue;
        };
        let Ok(valid) = deepref_documents::validate_external_url(&url) else {
            missing += 1;
            continue;
        };
        let mut tx = context.pool.begin().await?;
        let created = deepref_postgres::create_document(
            &mut tx,
            deepref_postgres::NewDocument {
                project_id: execution.project_id,
                report_id,
                id: Uuid::new_v4(),
                source: "external_url",
                status: "external",
                original_filename: None,
                external_url: Some(valid.as_str()),
                mime_type: "application/pdf",
                byte_size: 0,
                content_hash: None,
                object_key: None,
                actor_kind: actor.kind().as_str(),
                actor_id: actor.id(),
            },
        )
        .await;
        match created {
            Ok(document) => {
                if deepref_postgres::enqueue_retrieve(&mut tx, project(context), document.id)
                    .await
                    .is_ok()
                    && tx.commit().await.is_ok()
                {
                    attached += 1;
                } else {
                    missing += 1;
                }
            }
            Err(_) => {
                let _ = tx.rollback().await;
                missing += 1;
            }
        }
    }
    ok(
        "records",
        Value::Array(items),
        Some(format!(
            "Started downloading {attached} full text(s); {missing} could not be found."
        )),
    )
}

fn ris_escape(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

fn csv_escape(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

fn render_export(format: &str, items: &[Value]) -> (String, &'static str, &'static str) {
    let text = |item: &Value, key: &str| {
        item.get(key)
            .map(|value| match value {
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            })
            .unwrap_or_default()
    };
    let authors = |item: &Value| -> Vec<String> {
        item.get("authors")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    if format == "csv" {
        let mut out = String::from("title,authors,year,journal,doi,pmid,screening\n");
        for item in items {
            let row = [
                text(item, "title"),
                authors(item).join("; "),
                text(item, "year"),
                text(item, "journal"),
                text(item, "doi"),
                text(item, "pmid"),
                item.pointer("/screening/final")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            ];
            out.push_str(
                &row.iter()
                    .map(|cell| csv_escape(cell))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push('\n');
        }
        (out, "text/csv; charset=utf-8", "csv")
    } else {
        let mut out = String::new();
        for item in items {
            out.push_str("TY  - JOUR\n");
            for author in authors(item) {
                out.push_str(&format!("AU  - {}\n", ris_escape(&author)));
            }
            for (tag, key) in [
                ("TI", "title"),
                ("JO", "journal"),
                ("PY", "year"),
                ("DO", "doi"),
                ("AN", "pmid"),
                ("AB", "abstract"),
            ] {
                let value = text(item, key);
                if !value.is_empty() {
                    out.push_str(&format!("{tag}  - {}\n", ris_escape(&value)));
                }
            }
            out.push_str("ER  - \n\n");
        }
        (
            out,
            "application/x-research-info-systems; charset=utf-8",
            "ris",
        )
    }
}

async fn export_records(context: &Context<'_>) -> NodeResult {
    let execution = context.execution;
    let items = items_of(execution.inputs.get("records"));
    if items.is_empty() {
        return skipped("There were no records to export.");
    }
    let format = cfg_str(&execution.config, "format").unwrap_or("ris");
    let (content, content_type, extension) = render_export(format, &items);
    let stem: String = cfg_str(&execution.config, "name")
        .unwrap_or("records")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(80)
        .collect();
    let name = format!("{stem}.{extension}");
    if execution.test_mode {
        return ok(
            "file",
            json!({ "name": name, "records": items.len(), "dry_run": true }),
            dry_note(&format!(
                "Would have saved {name} with {} record(s).",
                items.len()
            )),
        );
    }
    let file_id = deepref_postgres::create_workflow_file(
        context.pool,
        execution.project_id,
        execution.run_id,
        &execution.node_id,
        &name,
        content_type,
        content.as_bytes(),
    )
    .await?;
    ok(
        "file",
        json!({ "file_id": file_id, "name": name, "records": items.len() }),
        Some(format!("Saved {name} with {} record(s).", items.len())),
    )
}

async fn ai_prompt(context: &Context<'_>) -> NodeResult {
    let execution = context.execution;
    let config = &execution.config;
    let data = execution.inputs.get("data").cloned().unwrap_or(Value::Null);
    let instructions = render_template(cfg_str(config, "instructions").unwrap_or_default(), &data);
    let answers = config
        .get("answers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut properties = Map::new();
    let mut required = Vec::new();
    for answer in &answers {
        let Some(name) = answer
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|n| !n.is_empty())
        else {
            continue;
        };
        let schema = match answer.get("kind").and_then(Value::as_str).unwrap_or("text") {
            "number" => json!({ "type": "number" }),
            "yes_no" => json!({ "type": "boolean" }),
            "list" => json!({ "type": "array", "items": { "type": "string" } }),
            _ => json!({ "type": "string" }),
        };
        let mut schema = schema;
        if let (Some(description), Value::Object(map)) = (
            answer.get("description").and_then(Value::as_str),
            &mut schema,
        ) {
            map.insert("description".to_owned(), json!(description));
        }
        properties.insert(name.to_owned(), schema);
        required.push(json!(name));
    }
    if properties.is_empty() {
        return Err(NodeError::permanent(
            "Add at least one answer for the AI to return.",
        ));
    }
    let gateway = context
        .ai
        .ok_or_else(|| NodeError::permanent("The AI is not set up on this server."))?;
    let store = PostgresAiStore::new(context.pool);
    let route = match store.resolve(ModelProfile::FastClassifier).await {
        Ok(route) => route,
        Err(_) => store
            .resolve(ModelProfile::Reasoning)
            .await
            .map_err(|_| NodeError::permanent("No AI model is switched on for this project."))?,
    };
    let mut payload = serde_json::to_string(&data).unwrap_or_default();
    payload.truncate(
        payload
            .char_indices()
            .nth(20_000)
            .map_or(payload.len(), |(index, _)| index),
    );
    let request = CompletionRequest {
        project_id: Some(project(context)),
        route,
        system_prompt: "You help with a systematic review. Answer only from the information given. Reply with JSON matching the requested schema.".to_owned(),
        user_prompt: format!("{instructions}\n\nInformation:\n{payload}"),
        evidence: Vec::new(),
        schema: json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false }),
    };
    let completion = gateway
        .complete(request)
        .await
        .map_err(|error| NodeError::retryable(format!("The AI could not answer: {error}")))?;
    let result: Value = serde_json::from_str(&completion.output_json)
        .map_err(|_| NodeError::retryable("The AI answered in a format that could not be used."))?;
    ok("result", result, None)
}

async fn schedule_reviews(context: &Context<'_>, task: &str, stage: &str) -> NodeResult {
    let execution = context.execution;
    let items: Vec<Value> = items_of(execution.inputs.get("records"))
        .into_iter()
        .take(50)
        .collect();
    if items.is_empty() {
        return skipped("There were no records for the AI to look at.");
    }
    let autonomy_task = match task {
        "screening" if stage == "full_text" => AutonomyTask::FullTextScreening,
        "screening" => AutonomyTask::TitleAbstractScreening,
        "appraisal_prefill" => AutonomyTask::Appraisal,
        "data_extraction" => AutonomyTask::Extraction,
        _ => AutonomyTask::StudyGrouping,
    };
    if autonomy(context, autonomy_task).await? == AutonomyLevel::Off {
        return skipped("The AI is switched off for this kind of work in this project.");
    }
    if execution.test_mode {
        return ok(
            "records",
            Value::Array(items.clone()),
            dry_note(&format!(
                "Would have asked the AI to review {} record(s).",
                items.len()
            )),
        );
    }
    let actor = automation_actor(context)?;
    let project_id = execution.project_id;
    let (mut scheduled, mut failed) = (0usize, 0usize);
    let mut first_error: Option<String> = None;
    for report_id in report_ids(&items) {
        let result: Result<(), deepref_postgres::ReviewPreparationError> = async {
            match task {
                "screening" => {
                    let ai_stage = if stage == "full_text" {
                        AiStage::FullText
                    } else {
                        AiStage::TitleAbstract
                    };
                    deepref_postgres::schedule_screening_review(
                        context.pool,
                        project_id,
                        report_id,
                        ai_stage,
                        None,
                        None,
                        actor.clone(),
                    )
                    .await?;
                }
                "study_grouping" => {
                    deepref_postgres::schedule_study_grouping_review(
                        context.pool,
                        project_id,
                        report_id,
                        actor.clone(),
                    )
                    .await?;
                }
                "appraisal_prefill" => {
                    let definitions =
                        deepref_application::all_appraisal_definitions().map_err(|error| {
                            deepref_postgres::ReviewPreparationError::InvalidInput(
                                error.to_string(),
                            )
                        })?;
                    let wanted = cfg_str(&execution.config, "appraisal_tool");
                    let definition = definitions
                        .iter()
                        .find(|definition| {
                            wanted.is_none_or(|wanted| definition.id.as_str() == wanted)
                        })
                        .ok_or_else(|| {
                            deepref_postgres::ReviewPreparationError::InvalidInput(
                                "The appraisal tool was not found.".to_owned(),
                            )
                        })?;
                    deepref_postgres::schedule_appraisal_prefill_review(
                        context.pool,
                        project_id,
                        report_id,
                        definition.id.as_str(),
                        definition.version.get(),
                        actor.clone(),
                    )
                    .await?;
                }
                "study_classification" | "data_extraction" => {
                    let study = deepref_postgres::study_of_report(
                        context.pool,
                        project(context),
                        report_id,
                    )
                    .await
                    .map_err(|error| {
                        deepref_postgres::ReviewPreparationError::InvalidInput(error.to_string())
                    })?;
                    let Some(study_id) = study
                        .as_ref()
                        .and_then(|study| study.get("study_id"))
                        .and_then(Value::as_str)
                        .and_then(|id| Uuid::parse_str(id).ok())
                    else {
                        return Err(deepref_postgres::ReviewPreparationError::InvalidInput(
                            "The record is not part of a study yet.".to_owned(),
                        ));
                    };
                    if task == "data_extraction" {
                        deepref_postgres::schedule_data_extraction_review(
                            context.pool,
                            project_id,
                            study_id,
                            actor.clone(),
                        )
                        .await?;
                    } else {
                        deepref_postgres::schedule_study_classification_review(
                            context.pool,
                            project_id,
                            study_id,
                            actor.clone(),
                        )
                        .await?;
                    }
                }
                _ => {
                    return Err(deepref_postgres::ReviewPreparationError::InvalidInput(
                        "This AI task is not available.".to_owned(),
                    ));
                }
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => scheduled += 1,
            Err(error) => {
                failed += 1;
                first_error.get_or_insert_with(|| error.to_string());
            }
        }
    }
    if scheduled == 0
        && let Some(error) = first_error
    {
        return Err(NodeError::permanent(format!(
            "The AI review could not be started: {error}"
        )));
    }
    ok(
        "records",
        Value::Array(items),
        Some(format!(
            "Started {scheduled} AI review(s); {failed} could not be started. Results appear as suggestions to review."
        )),
    )
}

/// Screening waits for the AI and sends every record out on the output that
/// matches its verdict. Each look at the reviews is a step job queued a short
/// time later, so no thread is held while the AI works and a restarted worker
/// picks the wait up where it was.
async fn screening_review(context: &Context<'_>, stage: &str) -> NodeResult {
    let execution = context.execution;
    let items = items_of(execution.inputs.get("records"));
    if items.is_empty() {
        return skipped("There were no records for the AI to look at.");
    }
    let mut wait = if let Some(saved) = &execution.wait_state {
        serde_json::from_value::<ScreeningWait>(saved.clone())
            .map_err(|_| NodeError::permanent("The AI reviews of this step could not be read."))?
    } else {
        // Checked once, when the reviews are started: a wait that is under way
        // keeps the setting it started with.
        let full_text = stage == "full_text";
        let autonomy_task = if full_text {
            AutonomyTask::FullTextScreening
        } else {
            AutonomyTask::TitleAbstractScreening
        };
        if autonomy(context, autonomy_task).await? == AutonomyLevel::Off {
            return skipped("The AI is switched off for this kind of work in this project.");
        }
        if execution.test_mode {
            return ok(
                "records",
                Value::Array(items.clone()),
                dry_note(&format!(
                    "Would have asked the AI to review {} record(s). A test run does not sort records by the AI's verdict.",
                    items.len()
                )),
            );
        }
        let budget = get_ai_budget(context.pool, execution.project_id)
            .await
            .map_err(|_| {
                NodeError::retryable("The AI budget could not be read. This will be tried again.")
            })?;
        if budget.exhausted() {
            let reviews: Vec<ReviewSlot> = reviewable_reports(&items)
                .into_iter()
                .map(|report_id| ReviewSlot {
                    report_id,
                    run_id: None,
                    verdict: Some(Verdict::stopped(BUDGET_USED_UP)),
                })
                .collect();
            return finish_screening(&items, &reviews, 0);
        }
        start_screening_reviews(context, &items, full_text).await?
    };
    settle_screening_reviews(context, &mut wait).await?;
    if !wait.is_over(Utc::now()) {
        wait.polls += 1;
        let total = wait.reviews.len();
        return Ok(NodeOutcome::Waiting {
            delay_secs: SCREENING_POLL_SECS,
            poll: wait.polls,
            note: progress_note(total - wait.open(), total),
            wait_state: serde_json::to_value(&wait).map_err(|_| {
                NodeError::permanent("The AI reviews of this step could not be saved.")
            })?,
        });
    }
    finish_screening(&items, &wait.reviews, wait.open())
}

const BUDGET_USED_UP: &str = "The AI budget for this project is used up for this month, so the AI did not review this record.";

/// The records one step sends to the AI: the first batch, each record once.
fn reviewable_reports(items: &[Value]) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = Vec::new();
    for item in items.iter().take(SCREENING_BATCH) {
        if let Some(id) = item
            .get("report_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// Start one screening review per record of the batch. A record whose review
/// cannot start is recorded with the reason. When none can start, the step
/// fails as it always has.
async fn start_screening_reviews(
    context: &Context<'_>,
    items: &[Value],
    full_text: bool,
) -> Result<ScreeningWait, NodeError> {
    let execution = context.execution;
    let actor = automation_actor(context)?;
    let stage = if full_text {
        AiStage::FullText
    } else {
        AiStage::TitleAbstract
    };
    let mut reviews = Vec::new();
    let mut started = 0usize;
    let mut first_error: Option<String> = None;
    for report_id in reviewable_reports(items) {
        match deepref_postgres::schedule_screening_review(
            context.pool,
            execution.project_id,
            report_id,
            stage,
            None,
            None,
            actor.clone(),
        )
        .await
        {
            Ok(run) => {
                started += 1;
                reviews.push(ReviewSlot {
                    report_id,
                    run_id: Some(run.id.as_uuid()),
                    verdict: None,
                });
            }
            Err(error) => {
                let message = error.to_string();
                first_error.get_or_insert_with(|| message.clone());
                reviews.push(ReviewSlot {
                    report_id,
                    run_id: None,
                    verdict: Some(Verdict::stopped(&format!(
                        "The AI review could not be started: {message}"
                    ))),
                });
            }
        }
    }
    if started == 0
        && let Some(error) = first_error
    {
        return Err(NodeError::permanent(format!(
            "The AI review could not be started: {error}"
        )));
    }
    Ok(ScreeningWait::start(Utc::now(), reviews))
}

/// Record the verdict of every review that has settled since the last look.
async fn settle_screening_reviews(
    context: &Context<'_>,
    wait: &mut ScreeningWait,
) -> Result<(), NodeError> {
    let execution = context.execution;
    let open: Vec<Uuid> = wait
        .reviews
        .iter()
        .filter(|slot| slot.verdict.is_none())
        .filter_map(|slot| slot.run_id)
        .collect();
    if open.is_empty() {
        return Ok(());
    }
    let runs = get_review_runs(context.pool, project(context), &open)
        .await
        .map_err(|_| {
            NodeError::retryable("The AI reviews could not be read. This will be tried again.")
        })?;
    for slot in wait
        .reviews
        .iter_mut()
        .filter(|slot| slot.verdict.is_none())
    {
        let Some(run_id) = slot.run_id else {
            continue;
        };
        let Some(run) = runs.iter().find(|run| run.id.as_uuid() == run_id) else {
            slot.verdict = Some(Verdict::stopped("The AI review could not be found."));
            continue;
        };
        slot.verdict = match &run.state {
            ReviewRunState::Queued | ReviewRunState::Running => None,
            ReviewRunState::Completed { proposal_id } => {
                match get_ai_proposal(context.pool, execution.project_id, *proposal_id).await {
                    Ok(proposal) => Some(Verdict::from_answer(&proposal.payload)),
                    Err(AiProposalError::Database(_)) => {
                        return Err(NodeError::retryable(
                            "The AI answer could not be read. This will be tried again.",
                        ));
                    }
                    Err(_) => Some(Verdict::stopped("The AI's answer could not be loaded.")),
                }
            }
            ReviewRunState::Blocked { message, .. } => Some(Verdict::stopped(&format!(
                "The AI could not finish this review: {}",
                truncate(message, 300)
            ))),
            ReviewRunState::Failed { message, .. } => {
                let reason = if message.to_lowercase().contains("budget") {
                    BUDGET_USED_UP.to_owned()
                } else {
                    format!("The AI review failed: {}", truncate(message, 300))
                };
                Some(Verdict::stopped(&reason))
            }
        };
    }
    Ok(())
}

/// Route every record by its verdict. `timed_out` counts the reviews that were
/// still open when the wait ended.
fn finish_screening(items: &[Value], reviews: &[ReviewSlot], timed_out: usize) -> NodeResult {
    let routed = Routed::route(judge_records(items.to_vec(), reviews));
    let note = summary_note(&routed, timed_out);
    let fired = routed.fired_ports();
    let mut outputs = Map::new();
    outputs.insert("records".to_owned(), Value::Array(routed.records));
    outputs.insert("included".to_owned(), Value::Array(routed.included));
    outputs.insert("excluded".to_owned(), Value::Array(routed.excluded));
    outputs.insert("unsure".to_owned(), Value::Array(routed.unsure));
    Ok(NodeOutcome::Completed {
        outputs,
        fired,
        items: 0,
        note: Some(note),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_identifiers_from_nested_data() {
        let data = json!({
            "subject": "x",
            "text": "see 10.1000/abc.def and PMID: 555",
            "records": [{"doi": "10.2000/Q", "pmid": "777"}],
        });
        let (mut dois, mut pmids) = (Vec::new(), Vec::new());
        collect_identifiers(&data, &mut dois, &mut pmids);
        assert!(dois.contains(&"10.1000/abc.def".to_owned()));
        assert!(dois.contains(&"10.2000/q".to_owned()));
        assert!(pmids.contains(&"555".to_owned()) && pmids.contains(&"777".to_owned()));
    }

    #[test]
    fn an_import_says_what_was_new_and_what_was_already_there() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        // Nothing with these identifiers was in the project; two records came back.
        assert_eq!(import_counts(&[], &[a, b]), (2, 0));
        // The DOI of `a` was already there: its copy is merged into the existing record.
        assert_eq!(import_counts(&[a], &[a]), (0, 1));
        assert_eq!(import_counts(&[a], &[a, b, c]), (2, 1));
        assert_eq!(
            import_note(0, 1, 0),
            "1 record(s) already in the project; nothing new was added."
        );
        assert_eq!(
            import_note(2, 1, 0),
            "Added 2 record(s); 1 already in the project."
        );
        assert_eq!(
            import_note(2, 0, 1),
            "Added 2 record(s). 1 DOI(s) could not be found."
        );
        assert_eq!(
            import_note(0, 0, 2),
            "No new record was added. 2 DOI(s) could not be found."
        );
    }

    #[test]
    fn exports_ris_and_csv() {
        let items = vec![
            json!({"title": "A, \"b\"", "authors": ["Li W"], "year": 2020, "doi": "10.1/x", "screening": {"final": "include"}}),
        ];
        let (ris, _, ext) = render_export("ris", &items);
        assert_eq!(ext, "ris");
        assert!(
            ris.contains("TI  - A, \"b\"") && ris.contains("AU  - Li W") && ris.contains("ER  - ")
        );
        let (csv, _, ext) = render_export("csv", &items);
        assert_eq!(ext, "csv");
        assert!(csv.contains("\"A, \"\"b\"\"\",\"Li W\",\"2020\""));
    }
}
