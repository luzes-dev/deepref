#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Blinding of the AI second reviewer and the provenance of exposures. Runs
//! against a real database and is skipped unless DATABASE_URL points at a
//! reachable PostgreSQL.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use deepref_application::{
    ProtocolCriterionCommand, PublishProtocolCommand, SaveProtocolDraftCommand, ScreenReportCommand,
};
use deepref_domain::{
    Actor, ActorKind, CriterionDimension, CriterionKind, CriterionStage, FrameworkKind, ProjectId,
    ProtocolVersionId, ReportId, ScreeningDecision, ScreeningStage,
};
use deepref_postgres::{
    ActivityFilters, AiProposalError, AiProposalFilters, AutonomyOutcome, ExposureSource,
    NewExposure, NewReviewerDecision, ProtocolActor, ResolveConflict, ReviewerError,
    apply_autonomy_for_proposal, delete_project_in_transaction, get_activity,
    get_published_protocol, get_visible_ai_proposal, independent_reviewer_pairs,
    insert_reviewer_decision_in_transaction, list_activity, list_ai_proposals,
    list_reviewer_decisions, migrate, publish_protocol, record_exposure, resolve_reviewer_conflict,
    reviewer_agreement, save_protocol_draft, screen_report,
};
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("DATABASE_URL must be reachable");
    migrate(&pool).await.expect("migrations must apply");
    Some(pool)
}

fn person() -> Actor {
    Actor::new(ActorKind::User, "tester").expect("actor")
}

async fn project(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'opinion exposure test')")
        .bind(id)
        .execute(pool)
        .await
        .expect("project");
    id
}

/// Publishes a protocol with one criterion and returns its version id.
async fn publish_protocol_for(pool: &PgPool, project_id: Uuid) -> Uuid {
    let actor = ProtocolActor {
        kind: "user".to_owned(),
        id: "opinion-exposure-test".to_owned(),
    };
    let draft = save_protocol_draft(
        pool,
        &SaveProtocolDraftCommand {
            project_id: ProjectId::new(project_id),
            protocol_version_id: None,
            name: "Exposure protocol".to_owned(),
            objective: "Blinding".to_owned(),
            question: "Does the person decide before the AI opinion is visible?".to_owned(),
            framework_kind: FrameworkKind::Pico,
            framework_fields: BTreeMap::from([
                (
                    "population".to_owned(),
                    "Adults with condition X".to_owned(),
                ),
                ("intervention".to_owned(), "Intervention Y".to_owned()),
                ("outcome".to_owned(), "Outcome Z".to_owned()),
            ]),
            criteria: vec![ProtocolCriterionCommand {
                id: None,
                kind: CriterionKind::Inclusion,
                stage: CriterionStage::Both,
                dimension: CriterionDimension::Population,
                label: "Population".to_owned(),
                description: "Adults with condition X".to_owned(),
            }],
            expected_revision: 0,
        },
        &actor,
    )
    .await
    .expect("draft saves");
    publish_protocol(
        pool,
        &PublishProtocolCommand {
            project_id: ProjectId::new(project_id),
            protocol_version_id: draft.id,
            expected_revision: draft.revision,
        },
        &actor,
    )
    .await
    .expect("protocol publishes");
    get_published_protocol(pool, project_id)
        .await
        .expect("published protocol")
        .id
}

async fn report(pool: &PgPool, project_id: Uuid, title: &str) -> Uuid {
    let report_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text) VALUES ($1,$2,'Adults with condition X.')",
    )
    .bind(report_id)
    .bind(title)
    .execute(pool)
    .await
    .expect("report");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(pool)
        .await
        .expect("membership");
    report_id
}

/// Stores an AI second-reviewer opinion for one record and stage, without a
/// proposal, and returns the decision id.
async fn ai_opinion(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    stage: &str,
    decision: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    let mut tx = pool.begin().await.expect("tx");
    insert_reviewer_decision_in_transaction(
        &mut tx,
        &NewReviewerDecision {
            id,
            project_id,
            report_id,
            stage: stage.to_owned(),
            decision: decision.to_owned(),
            rationale: "Fixture rationale.".to_owned(),
            evidence: json!([{"label": "Abstract", "quote": "adults with condition X"}]),
            source: "ai",
            proposal_id: None,
            ai_run_id: None,
            model: Some("exposure-test-model".to_owned()),
            prompt_version: None,
            activity_id: None,
        },
    )
    .await
    .expect("opinion inserts");
    tx.commit().await.expect("commit");
    id
}

fn hex64() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// A pending screening suggestion for one record and stage, as the screening
/// review writes it. Returns the proposal id and its AI run id.
async fn screening_proposal(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    protocol_id: Uuid,
    stage: &str,
    kind: &str,
) -> (Uuid, Uuid) {
    let run_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ai_runs (id, project_id, task_kind, provider, model, prompt_version, input_hash,
           status, profile, model_version, schema_version, prompt_hash, schema_hash, reuse_hash,
           input_tokens, output_tokens)
         VALUES ($1,$2,'screening_suggestion','exposure-test','reasoner','v1',$3,
                 'completed','standard','2026-08','v1',$4,$5,$6,0,0)",
    )
    .bind(run_id)
    .bind(project_id)
    .bind(hex64())
    .bind(hex64())
    .bind(hex64())
    .bind(hex64())
    .execute(pool)
    .await
    .expect("AI run");
    sqlx::query(
        "INSERT INTO ai_proposals
           (id, project_id, ai_run_id, model_run_id, proposal_type, payload, status, entity_type,
            entity_id, operation, authority_tier, task_kind, target_report_id, protocol_version_id,
            expected_revision)
         VALUES ($1,$2,$3,$3,'screening_suggestion',$4,'pending','screening_report',$5,
                 'screening_suggestion','scientific_conclusion','screening_suggestion',$5,$6,0)",
    )
    .bind(proposal_id)
    .bind(project_id)
    .bind(run_id)
    .bind(json!({
        "report_id": report_id,
        "protocol_version_id": protocol_id,
        "stage": stage,
        "expected_revision": 0,
        "criteria": [],
        "suggested_decision": {"kind": kind},
        "uncertainties": [],
    }))
    .bind(report_id)
    .bind(protocol_id)
    .execute(pool)
    .await
    .expect("screening proposal");
    (proposal_id, run_id)
}

/// The person's screening decision on a record at title and abstract.
async fn decide(pool: &PgPool, project_id: Uuid, report_id: Uuid, decision: ScreeningDecision) {
    let protocol_id = get_published_protocol(pool, project_id)
        .await
        .expect("protocol")
        .id;
    let revision: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT revision FROM screening_state WHERE project_id=$1 AND report_id=$2),0)",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(pool)
    .await
    .expect("revision");
    screen_report(
        pool,
        ScreenReportCommand {
            project_id: ProjectId::new(project_id),
            report_id: ReportId::new(report_id),
            stage: ScreeningStage::TitleAbstract,
            decision,
            exclusion_reason_id: None,
            protocol_version_id: ProtocolVersionId::new(protocol_id),
            expected_revision: revision,
            notes: None,
            actor: person(),
        },
    )
    .await
    .expect("decision recorded");
}

async fn exposures_from(pool: &PgPool, project_id: Uuid, source: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM ai_opinion_exposures WHERE project_id=$1 AND exposure_source=$2",
    )
    .bind(project_id)
    .bind(source)
    .fetch_one(pool)
    .await
    .expect("exposure count")
}

async fn all_exposures(pool: &PgPool, project_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM ai_opinion_exposures WHERE project_id=$1")
        .bind(project_id)
        .fetch_one(pool)
        .await
        .expect("exposure count")
}

async fn title_abstract_agreement(
    pool: &PgPool,
    project_id: Uuid,
) -> deepref_postgres::StageAgreement {
    reviewer_agreement(pool, project_id)
        .await
        .expect("agreement")
        .into_iter()
        .find(|(stage, _)| stage == "title_abstract")
        .expect("title and abstract")
        .1
}

#[tokio::test]
async fn a_waiting_second_opinion_stays_blind_on_every_read_path() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    let protocol_id = publish_protocol_for(&pool, project_id).await;
    let record = report(&pool, project_id, "Blind record").await;
    let (proposal_id, run_id) = screening_proposal(
        &pool,
        project_id,
        record,
        protocol_id,
        "title_abstract",
        "exclude",
    )
    .await;
    assert!(
        matches!(
            get_visible_ai_proposal(&pool, project_id, proposal_id).await,
            Err(AiProposalError::NotFound)
        ),
        "pending proposal stays blind before autonomy processing"
    );
    assert_eq!(all_exposures(&pool, project_id).await, 0);
    // The project's default setting makes the AI a second reviewer for screening.
    let outcome = apply_autonomy_for_proposal(&pool, project_id, proposal_id)
        .await
        .expect("autonomy");
    assert!(matches!(outcome, AutonomyOutcome::SecondReviewer { .. }));

    let feed = list_activity(&pool, project_id, ActivityFilters::default(), None, 50)
        .await
        .expect("feed");
    let entry = feed
        .iter()
        .find(|item| item.action == "second_reviewer_opinion")
        .expect("the opinion entry");
    assert_eq!(
        entry.summary,
        "The AI recorded an independent second opinion on “Blind record”. It stays hidden until you record your own decision."
    );
    assert!(entry.evidence.as_array().is_some_and(Vec::is_empty));
    assert_eq!(entry.proposal_id, None);
    assert_eq!(entry.ai_run_id, None);
    let fetched = get_activity(&pool, project_id, entry.id)
        .await
        .expect("entry by id");
    assert_eq!(fetched.summary, entry.summary);
    assert_eq!(fetched.proposal_id, None);
    assert_eq!(fetched.ai_run_id, None);

    let waiting = list_reviewer_decisions(&pool, project_id, None, Some("waiting"), 10)
        .await
        .expect("waiting list");
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].ai_decision, "");
    assert_eq!(waiting[0].ai_rationale, "");

    let expired = AiProposalFilters {
        status: Some("expired"),
        task_kind: None,
        target_report_id: Some(record),
        target_record_id: None,
        candidate_report_id: None,
        target_study_id: None,
    };
    assert!(
        list_ai_proposals(&pool, project_id, expired, None, 10)
            .await
            .expect("proposals")
            .is_empty()
    );
    assert!(matches!(
        get_visible_ai_proposal(&pool, project_id, proposal_id).await,
        Err(AiProposalError::NotFound)
    ));
    // Reads that withheld the opinion recorded no exposure.
    assert_eq!(all_exposures(&pool, project_id).await, 0);

    // Once the person has decided, the entry shows what the AI said.
    decide(&pool, project_id, record, ScreeningDecision::Include).await;
    let feed = list_activity(&pool, project_id, ActivityFilters::default(), None, 50)
        .await
        .expect("feed after the decision");
    let entry = feed
        .iter()
        .find(|item| item.action == "second_reviewer_opinion")
        .expect("the opinion entry");
    assert!(entry.summary.contains("would exclude"), "{}", entry.summary);
    assert_eq!(entry.proposal_id, Some(proposal_id));
    assert_eq!(entry.ai_run_id, Some(run_id));
    let visible = get_visible_ai_proposal(&pool, project_id, proposal_id)
        .await
        .expect("visible after the decision");
    assert_eq!(visible.status, "expired");
    let mut tx = pool.begin().await.expect("tx");
    delete_project_in_transaction(&mut tx, project_id)
        .await
        .expect("cleanup");
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn an_exposure_before_the_decision_excludes_the_pair() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    let exposed = report(&pool, project_id, "Exposed before the decision").await;
    let clean = report(&pool, project_id, "Never exposed").await;
    ai_opinion(&pool, project_id, exposed, "title_abstract", "include").await;
    ai_opinion(&pool, project_id, clean, "title_abstract", "include").await;

    // A workflow run output carried the verdict for one record before the person decided.
    record_exposure(
        &pool,
        &NewExposure::new(
            project_id,
            exposed,
            "title_abstract",
            ExposureSource::WorkflowRunOutput,
        ),
    )
    .await
    .expect("exposure");
    decide(&pool, project_id, exposed, ScreeningDecision::Include).await;
    decide(&pool, project_id, clean, ScreeningDecision::Exclude).await;

    let pairs = independent_reviewer_pairs(&pool, project_id, Some("title_abstract"))
        .await
        .expect("pairs");
    assert_eq!(pairs.exposed_excluded, 1);
    assert_eq!(pairs.unverifiable_excluded, 0);
    assert_eq!(pairs.independent.len(), 1);
    assert_eq!(pairs.independent[0].report_id, clean);
    assert_eq!(pairs.independent[0].human_decision, "exclude");
    assert_eq!(pairs.independent[0].ai_decision, "include");

    let agreement = title_abstract_agreement(&pool, project_id).await;
    assert_eq!(agreement.compared, 1);
    assert_eq!(agreement.agreed, 0);
    assert_eq!(agreement.excluded_exposed, 1);
}

#[tokio::test]
async fn an_exposure_after_the_decision_or_for_someone_else_does_not_exclude() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;

    // Decided first, the verdict shown afterwards: the decision was independent.
    let decided_first = report(&pool, project_id, "Decided first").await;
    ai_opinion(
        &pool,
        project_id,
        decided_first,
        "title_abstract",
        "include",
    )
    .await;
    decide(&pool, project_id, decided_first, ScreeningDecision::Include).await;
    record_exposure(
        &pool,
        &NewExposure::new(
            project_id,
            decided_first,
            "title_abstract",
            ExposureSource::WorkflowRunOutput,
        ),
    )
    .await
    .expect("late exposure");

    // Shown to someone else before the decision: this person decided blind.
    let other_person = report(&pool, project_id, "Shown to someone else").await;
    ai_opinion(&pool, project_id, other_person, "title_abstract", "include").await;
    let mut for_someone_else = NewExposure::new(
        project_id,
        other_person,
        "title_abstract",
        ExposureSource::WorkflowRunOutput,
    );
    for_someone_else.audience = Some(Actor::new(ActorKind::User, "someone-else").expect("actor"));
    record_exposure(&pool, &for_someone_else)
        .await
        .expect("exposure for someone else");
    decide(&pool, project_id, other_person, ScreeningDecision::Include).await;

    // Shown to the person who decides, before the decision: exposed.
    let shown_to_person = report(&pool, project_id, "Shown to this person").await;
    ai_opinion(
        &pool,
        project_id,
        shown_to_person,
        "title_abstract",
        "include",
    )
    .await;
    let mut for_this_person = NewExposure::new(
        project_id,
        shown_to_person,
        "title_abstract",
        ExposureSource::WorkflowRunOutput,
    );
    for_this_person.audience = Some(Actor::new(ActorKind::User, "tester").expect("actor"));
    record_exposure(&pool, &for_this_person)
        .await
        .expect("exposure for this person");
    decide(
        &pool,
        project_id,
        shown_to_person,
        ScreeningDecision::Include,
    )
    .await;

    let pairs = independent_reviewer_pairs(&pool, project_id, None)
        .await
        .expect("pairs");
    let independent: Vec<Uuid> = pairs
        .independent
        .iter()
        .map(|pair| pair.report_id)
        .collect();
    assert!(independent.contains(&decided_first));
    assert!(independent.contains(&other_person));
    assert!(!independent.contains(&shown_to_person));
    assert_eq!(pairs.exposed_excluded, 1);
    assert_eq!(pairs.unverifiable_excluded, 0);
}

#[tokio::test]
async fn resolving_needs_a_decision_and_keeps_the_time_it_replaced() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;

    // The person has not decided: the opinion stays hidden and nothing is recorded.
    let undecided = report(&pool, project_id, "Still blind").await;
    let undecided_opinion =
        ai_opinion(&pool, project_id, undecided, "title_abstract", "include").await;
    let refused = resolve_reviewer_conflict(
        &pool,
        project_id,
        undecided_opinion,
        ResolveConflict {
            decision: ScreeningDecision::Include,
            exclusion_reason_id: None,
            note: None,
        },
        &person(),
    )
    .await;
    assert!(matches!(refused, Err(ReviewerError::NotDecided)));
    assert_eq!(all_exposures(&pool, project_id).await, 0);

    // A conflict the person has settled: the replaced decision's time is stored, and
    // the reveal is recorded.
    let conflict = report(&pool, project_id, "Settled conflict").await;
    let opinion = ai_opinion(&pool, project_id, conflict, "title_abstract", "include").await;
    decide(&pool, project_id, conflict, ScreeningDecision::Exclude).await;
    let settled = resolve_reviewer_conflict(
        &pool,
        project_id,
        opinion,
        ResolveConflict {
            decision: ScreeningDecision::Include,
            exclusion_reason_id: None,
            note: Some("Population matches.".to_owned()),
        },
        &person(),
    )
    .await
    .expect("resolution");
    assert_eq!(settled.status, "resolved");
    assert_eq!(settled.resolution.as_deref(), Some("adopted_ai"));

    let replaced_at: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT human_decision_before_at FROM ai_reviewer_decisions WHERE id=$1",
    )
    .bind(opinion)
    .fetch_one(&pool)
    .await
    .expect("stored time");
    let first_user_event: DateTime<Utc> = sqlx::query_scalar(
        "SELECT created_at FROM screening_events
         WHERE project_id=$1 AND report_id=$2 AND actor_kind='user'
         ORDER BY created_at ASC, id ASC LIMIT 1",
    )
    .bind(project_id)
    .bind(conflict)
    .fetch_one(&pool)
    .await
    .expect("first decision");
    assert_eq!(replaced_at, Some(first_user_event));
    let reveals: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_opinion_exposures
         WHERE ai_reviewer_decision_id=$1 AND exposure_source='reviewer_opinion_reveal'",
    )
    .bind(opinion)
    .fetch_one(&pool)
    .await
    .expect("reveal count");
    assert_eq!(reveals, 1);

    // The pair still counts: the person decided before the opinion was revealed.
    let pairs = independent_reviewer_pairs(&pool, project_id, Some("title_abstract"))
        .await
        .expect("pairs");
    let pair = pairs
        .independent
        .iter()
        .find(|pair| pair.report_id == conflict)
        .expect("the resolved pair still counts");
    assert_eq!(pair.human_decision, "exclude");
    assert_eq!(pair.ai_decision, "include");
}

#[tokio::test]
async fn a_decision_changed_after_the_opinion_was_shown_is_exposed() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    let record = report(&pool, project_id, "Changed later").await;
    let opinion = ai_opinion(&pool, project_id, record, "title_abstract", "include").await;
    decide(&pool, project_id, record, ScreeningDecision::Include).await;

    // The opinion is returned now that the person has decided. The view is recorded
    // once, however many times it is listed.
    for _ in 0..2 {
        let shown = list_reviewer_decisions(&pool, project_id, None, Some("concordant"), 10)
            .await
            .expect("list");
        assert_eq!(shown.len(), 1);
    }
    let reveals: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_opinion_exposures
         WHERE ai_reviewer_decision_id=$1 AND exposure_source='reviewer_opinion_reveal'",
    )
    .bind(opinion)
    .fetch_one(&pool)
    .await
    .expect("reveal count");
    assert_eq!(reveals, 1);

    // Blind decision and no later view: the pair is independent.
    let before = independent_reviewer_pairs(&pool, project_id, None)
        .await
        .expect("pairs");
    assert_eq!(before.independent.len(), 1);
    assert_eq!(before.exposed_excluded, 0);

    // The person changes the decision after having seen the AI's opinion.
    decide(&pool, project_id, record, ScreeningDecision::Exclude).await;
    let after = independent_reviewer_pairs(&pool, project_id, None)
        .await
        .expect("pairs after the change");
    assert!(after.independent.is_empty());
    assert_eq!(after.exposed_excluded, 1);
}

#[tokio::test]
async fn listing_a_pending_suggestion_records_the_exposure_once() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    let protocol_id = publish_protocol_for(&pool, project_id).await;
    let record = report(&pool, project_id, "Suggested record").await;
    let (proposal_id, _) = screening_proposal(
        &pool,
        project_id,
        record,
        protocol_id,
        "title_abstract",
        "include",
    )
    .await;
    deepref_postgres::set_autonomy_level(
        &pool,
        project_id,
        deepref_application::workflows::AutonomyTask::TitleAbstractScreening,
        deepref_application::workflows::AutonomyLevel::Suggest,
        &person(),
    )
    .await
    .expect("suggest mode");
    assert_eq!(
        exposures_from(&pool, project_id, "screening_suggestion").await,
        1,
        "availability is recorded before any proposal is fetched"
    );
    let pending = AiProposalFilters {
        status: Some("pending"),
        task_kind: None,
        target_report_id: Some(record),
        target_record_id: None,
        candidate_report_id: None,
        target_study_id: None,
    };
    for _ in 0..2 {
        let listed = list_ai_proposals(&pool, project_id, pending, None, 10)
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);
    }
    get_visible_ai_proposal(&pool, project_id, proposal_id)
        .await
        .expect("suggestion is visible in suggest mode");

    let recorded: Vec<(String, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT report_id::text, stage, proposal_id FROM ai_opinion_exposures
         WHERE project_id=$1 AND exposure_source='screening_suggestion'",
    )
    .bind(project_id)
    .fetch_all(&pool)
    .await
    .expect("suggestion exposures");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, record.to_string());
    assert_eq!(recorded[0].1, "title_abstract");
    assert_eq!(recorded[0].2, Some(proposal_id));
}

#[tokio::test]
async fn an_opinion_the_feed_showed_after_the_decision_makes_a_later_change_exposed() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    let protocol_id = publish_protocol_for(&pool, project_id).await;
    let record = report(&pool, project_id, "Shown after the decision").await;
    let (proposal_id, _) = screening_proposal(
        &pool,
        project_id,
        record,
        protocol_id,
        "title_abstract",
        "exclude",
    )
    .await;
    apply_autonomy_for_proposal(&pool, project_id, proposal_id)
        .await
        .expect("autonomy");
    // The person decides first, without the opinion being visible.
    decide(&pool, project_id, record, ScreeningDecision::Include).await;
    let before = independent_reviewer_pairs(&pool, project_id, None)
        .await
        .expect("pairs");
    assert_eq!(before.independent.len(), 1);

    // Now that the person has decided, the feed shows the opinion. That view is recorded,
    // once, however often the feed is read.
    for _ in 0..2 {
        let feed = list_activity(&pool, project_id, ActivityFilters::default(), None, 50)
            .await
            .expect("feed");
        let entry = feed
            .iter()
            .find(|item| item.action == "second_reviewer_opinion")
            .expect("the opinion entry");
        assert!(entry.summary.contains("would exclude"), "{}", entry.summary);
    }
    assert_eq!(
        exposures_from(&pool, project_id, "reviewer_opinion_reveal").await,
        1
    );

    // The person changes the decision after having seen the opinion: exposed.
    decide(&pool, project_id, record, ScreeningDecision::Exclude).await;
    let after = independent_reviewer_pairs(&pool, project_id, None)
        .await
        .expect("pairs after the change");
    assert!(after.independent.is_empty());
    assert_eq!(after.exposed_excluded, 1);
}

#[tokio::test]
async fn exposures_cannot_be_edited_and_a_project_delete_still_removes_them() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    let record = report(&pool, project_id, "Immutable record").await;
    let opinion = ai_opinion(&pool, project_id, record, "title_abstract", "include").await;
    let mut exposure = NewExposure::new(
        project_id,
        record,
        "title_abstract",
        ExposureSource::WorkflowRunOutput,
    );
    exposure.ai_reviewer_decision_id = Some(opinion);
    record_exposure(&pool, &exposure).await.expect("exposure");

    let update = sqlx::query("UPDATE ai_opinion_exposures SET stage = stage WHERE project_id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect_err("UPDATE must be refused");
    assert!(
        update.to_string().contains("append-only"),
        "unexpected error: {update}"
    );
    assert_eq!(all_exposures(&pool, project_id).await, 1);

    // Deleting the project cascades through the decision and the exposure without an UPDATE.
    let mut tx = pool.begin().await.expect("tx");
    assert!(
        delete_project_in_transaction(&mut tx, project_id)
            .await
            .expect("project delete")
    );
    tx.commit().await.expect("commit");
    assert_eq!(all_exposures(&pool, project_id).await, 0);
    assert_eq!(
        exposures_from(&pool, project_id, "workflow_run_output").await,
        0
    );
}

#[tokio::test]
async fn a_suggestion_available_before_screening_contaminates_without_a_page_open() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    let protocol_id = publish_protocol_for(&pool, project_id).await;
    deepref_postgres::set_autonomy_level(
        &pool,
        project_id,
        deepref_application::workflows::AutonomyTask::TitleAbstractScreening,
        deepref_application::workflows::AutonomyLevel::Suggest,
        &person(),
    )
    .await
    .expect("suggest mode");
    let record = report(&pool, project_id, "Available suggestion").await;
    screening_proposal(
        &pool,
        project_id,
        record,
        protocol_id,
        "title_abstract",
        "include",
    )
    .await;
    assert_eq!(
        exposures_from(&pool, project_id, "screening_suggestion").await,
        1
    );
    ai_opinion(&pool, project_id, record, "title_abstract", "include").await;
    decide(&pool, project_id, record, ScreeningDecision::Include).await;
    let pairs = independent_reviewer_pairs(&pool, project_id, None)
        .await
        .expect("pairs");
    assert!(pairs.independent.is_empty());
    assert_eq!(pairs.exposed_excluded, 1);
    let rows = deepref_postgres::load_audit_export_rows(&pool, project_id, 1000)
        .await
        .expect("audit export");
    assert!(rows.iter().any(|row| {
        row.event_type == "ai_opinion_exposure"
            && row
                .payload
                .get("exposure_source")
                .and_then(serde_json::Value::as_str)
                == Some("screening_suggestion")
    }));
}

#[tokio::test]
async fn incomplete_new_calibration_snapshots_are_rejected_by_the_database() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    for snapshot in [
        json!({}),
        json!({"scheme": 2, "definition": "screening", "stage": "title_abstract"}),
    ] {
        let result = sqlx::query(
            "INSERT INTO review_calibration_bundles
             (id,project_id,definition_key,stage,identity_scheme,identity_snapshot,
              semantic_bundle_hash,evaluation_set_id,thresholds,metrics,reviewer_metadata,status,evaluated_at)
             VALUES ($1,$2,'screening','title_abstract',2,$3,$4,'invalid-snapshot','{}','{}','{}','passing',now())"
        ).bind(Uuid::new_v4()).bind(project_id).bind(snapshot).bind(hex64()).execute(&pool).await;
        assert!(result.is_err(), "missing identity fields must fail closed");
        assert_eq!(
            result
                .err()
                .and_then(|e| e
                    .as_database_error()
                    .and_then(|db| db.code().map(|c| c.to_string())))
                .as_deref(),
            Some("23514")
        );
    }
}

#[tokio::test]
async fn an_automation_state_cannot_be_attributed_to_an_earlier_human_decision() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    let protocol_id = publish_protocol_for(&pool, project_id).await;
    let record = report(&pool, project_id, "Automation changed this record").await;
    decide(&pool, project_id, record, ScreeningDecision::Include).await;
    screen_report(
        &pool,
        ScreenReportCommand {
            project_id: ProjectId::new(project_id),
            report_id: ReportId::new(record),
            stage: ScreeningStage::TitleAbstract,
            decision: ScreeningDecision::Exclude,
            exclusion_reason_id: None,
            protocol_version_id: ProtocolVersionId::new(protocol_id),
            expected_revision: 1,
            notes: None,
            actor: Actor::new(ActorKind::Automation, "test-workflow").expect("automation"),
        },
    )
    .await
    .expect("automation decision");
    ai_opinion(&pool, project_id, record, "title_abstract", "exclude").await;
    let pairs = independent_reviewer_pairs(&pool, project_id, None)
        .await
        .expect("pairs");
    assert!(
        pairs.independent.is_empty(),
        "automation is not a human reference label"
    );
    assert_eq!(pairs.unverifiable_excluded, 1);
}
