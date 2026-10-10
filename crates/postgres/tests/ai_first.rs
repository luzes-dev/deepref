#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use chrono::Utc;
use deepref_ai::{ModelParameters, ModelProfile, ResolvedModel};
use deepref_application::{
    GetScreeningQueueQuery, ProtocolCriterionCommand, PublishProtocolCommand,
    SaveProtocolDraftCommand, ScreenReportCommand, ScreeningQueueSort, ScreeningQueueStatus,
    UndoScreeningCommand,
};
use deepref_domain::{
    Actor, ActorKind, CriterionDimension, CriterionKind, CriterionStage, FrameworkKind,
    ScreeningStage,
};
use deepref_postgres::*;
use deepref_review::{
    ReviewDefinitionKey, ReviewOrigin, ReviewScheduler, ReviewSubject, ScheduleReviewRun,
};
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Same database-wide key as `review_runs.rs` so route inserts serialize even
/// when both binaries share a database. Transaction-scoped: released when the
/// returned transaction commits or rolls back.
const ROUTE_FIXTURE_LOCK: i64 = 0x5245_5649_4557_0001;

async fn route_fixture_lock(pool: &PgPool) -> Transaction<'static, Postgres> {
    let mut transaction = pool.begin().await.expect("route lock transaction begins");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ROUTE_FIXTURE_LOCK)
        .execute(&mut *transaction)
        .await
        .expect("route fixture lock is acquired");
    transaction
}

/// Every test inserts the identical global route, so concurrent inserts collide
/// on the profile/provider/model/version unique key. Serialize just the insert;
/// the routes are identical, so the resolved identity is stable afterwards and
/// the rest of the fixture can run in parallel.
async fn insert_fixture_route(pool: &PgPool) {
    let lock = route_fixture_lock(pool).await;
    insert_model_route(
        pool,
        &ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: "ai-first-fixture".into(),
            model: "test-model".into(),
            model_version: "immutable-test-v1".into(),
            parameters: ModelParameters::default(),
            route_id: None,
        },
        Utc::now(),
    )
    .await
    .unwrap();
    lock.commit().await.unwrap();
}

fn actor(id: &str) -> Actor {
    Actor::new(ActorKind::User, id).unwrap()
}
async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .connect(&url)
        .await
        .expect("database connection");
    migrate(&pool).await.expect("migrations");
    Some(pool)
}
struct Fixture {
    pool: PgPool,
    project: Uuid,
    cohort: Uuid,
    protocol: Uuid,
    x: Vec<Uuid>,
    human: Vec<Uuid>,
}
impl Fixture {
    async fn new(pool: PgPool, allow_finalization: bool) -> Self {
        let project = Uuid::new_v4();
        sqlx::query("INSERT INTO projects(id,name) VALUES($1,'AI first test')")
            .bind(project)
            .execute(&pool)
            .await
            .unwrap();
        insert_fixture_route(&pool).await;
        let protocol = Self::publish(&pool, project).await;
        let mut x = Vec::new();
        let mut human = Vec::new();
        for i in 0..60 {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO reports(id,title,abstract_text) VALUES($1,$2,$3)")
                .bind(id).bind(format!("Study {i}")).bind("Randomized study of adults with condition X and an intervention. Sufficient abstract evidence.")
                .execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
                .bind(project)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            if i < 20 {
                human.push(id);
            } else {
                x.push(id);
            }
        }
        let cohort = start_ai_first_cohort(&pool, project, 95, allow_finalization, &actor("owner"))
            .await
            .unwrap();
        // Seed validated exclusions through trusted storage fixtures. Routing itself
        // is exercised by compiled-worker tests; these tests isolate the audit lifecycle.
        for id in &x {
            let run = PostgresReviewScheduler::new(&pool)
                .schedule(ScheduleReviewRun {
                    project_id: project.into(),
                    definition: ReviewDefinitionKey::Screening,
                    subject: ReviewSubject::Screening {
                        report_id: (*id).into(),
                        stage: ScreeningStage::TitleAbstract,
                        protocol_version_id: protocol.into(),
                        expected_revision: 0,
                    },
                    origin: ReviewOrigin::AiFirstTriggered { cohort_id: cohort },
                    actor: Actor::new(ActorKind::Automation, "ai-first-fixture").unwrap(),
                })
                .await
                .unwrap();
            let ai = Uuid::new_v4();
            let hash = deepref_ai::sha256_bytes(ai.as_bytes());
            sqlx::query("INSERT INTO ai_runs(id,project_id,task_kind,provider,model,prompt_version,input_hash,status,profile,model_version,schema_version,prompt_hash,schema_hash,reuse_hash,input_tokens,output_tokens)
              VALUES($1,$2,'screening_suggestion','ai-first-fixture','test-model','v1',$3,'completed','standard','v1','v1',$3,$3,$3,0,0)")
                .bind(ai).bind(project).bind(hash).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO ai_screening_dispositions(project_id,cohort_id,report_id,review_run_id,ai_run_id,evaluated_revision,
               protocol_version_id,semantic_bundle_hash,policy_version,source_snapshot) SELECT $1,$2,$3,$4,$5,0,$6,c.semantic_bundle_hash,1,
               jsonb_build_object('title',r.title,'abstract_text',r.abstract_text) FROM ai_screening_cohorts c,reports r WHERE c.id=$2 AND r.id=$3")
                .bind(project).bind(cohort).bind(id).bind(run.id.as_uuid()).bind(ai).bind(protocol).execute(&pool).await.unwrap();
        }
        let f = Self {
            pool,
            project,
            cohort,
            protocol,
            x,
            human,
        };
        for id in &f.human {
            f.decide(
                *id,
                "reviewer-initial",
                deepref_domain::ScreeningDecision::Include,
            )
            .await
            .unwrap();
        }
        f
    }
    async fn publish(pool: &PgPool, project: Uuid) -> Uuid {
        let a = ProtocolActor {
            kind: "user".into(),
            id: "owner".into(),
        };
        let expected_revision: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(revision),0)::bigint FROM protocol_versions WHERE project_id=$1 AND status='published'")
            .bind(project).fetch_one(pool).await.unwrap();
        let draft = save_protocol_draft(
            pool,
            &SaveProtocolDraftCommand {
                project_id: project.into(),
                protocol_version_id: None,
                name: "AI-first protocol".into(),
                objective: "Reference retention".into(),
                question: "Adults with condition X?".into(),
                framework_kind: FrameworkKind::Pico,
                framework_fields: BTreeMap::from([
                    ("population".into(), "Adults".into()),
                    ("intervention".into(), "Y".into()),
                    ("outcome".into(), "Z".into()),
                ]),
                criteria: vec![ProtocolCriterionCommand {
                    id: None,
                    kind: CriterionKind::Inclusion,
                    stage: CriterionStage::Both,
                    dimension: CriterionDimension::Population,
                    label: "Adults".into(),
                    description: "Adults with condition X".into(),
                }],
                expected_revision,
            },
            &a,
        )
        .await
        .unwrap();
        publish_protocol(
            pool,
            &PublishProtocolCommand {
                project_id: project.into(),
                protocol_version_id: draft.id,
                expected_revision: draft.revision,
            },
            &a,
        )
        .await
        .unwrap()
        .id
    }
    async fn revision(&self, report: Uuid) -> i64 {
        sqlx::query_scalar("SELECT COALESCE((SELECT revision FROM screening_state WHERE project_id=$1 AND report_id=$2),0)::bigint")
            .bind(self.project).bind(report).fetch_one(&self.pool).await.unwrap()
    }
    async fn decide(
        &self,
        report: Uuid,
        id: &str,
        decision: deepref_domain::ScreeningDecision,
    ) -> Result<ScreeningStateSnapshot, ScreeningError> {
        screen_report(
            &self.pool,
            ScreenReportCommand {
                project_id: self.project.into(),
                report_id: report.into(),
                stage: ScreeningStage::TitleAbstract,
                decision,
                exclusion_reason_id: None,
                protocol_version_id: self.protocol.into(),
                expected_revision: if self.status().await == "auditing" {
                    0
                } else {
                    self.revision(report).await
                },
                notes: None,
                actor: actor(id),
            },
        )
        .await
    }
    async fn draw(&self) -> Vec<Uuid> {
        close_ai_first_cohort(&self.pool, self.project, self.cohort, &actor("owner"))
            .await
            .unwrap();
        let n = draw_ai_first_audit(&self.pool, self.project, self.cohort, None, &actor("owner"))
            .await
            .unwrap();
        let ids=sqlx::query_scalar("SELECT report_id FROM ai_screening_cohort_members WHERE cohort_id=$1 AND sampled ORDER BY report_id")
            .bind(self.cohort).fetch_all(&self.pool).await.unwrap();
        assert_eq!(ids.len(), n as usize);
        assert!(n > 0 && n < 40);
        ids
    }
    async fn label_all(&self, ids: &[Uuid], miss: bool) {
        let controls: Vec<Uuid>=sqlx::query_scalar("SELECT report_id FROM ai_screening_cohort_members WHERE cohort_id=$1 AND audit_control ORDER BY report_id")
            .bind(self.cohort).fetch_all(&self.pool).await.unwrap();
        let tasks: Vec<_> = ids.iter().chain(controls.iter()).collect();
        for (i, id) in tasks.iter().enumerate() {
            let decision = if i >= ids.len() {
                deepref_domain::ScreeningDecision::Include
            } else if miss && i == 0 {
                deepref_domain::ScreeningDecision::Maybe
            } else {
                deepref_domain::ScreeningDecision::Exclude
            };
            let first = self.decide(**id, "reviewer-a", decision).await.unwrap();
            assert_eq!(first.revision, 0);
            assert_eq!(first.title_abstract_status, "unscreened");
            assert!(self.decide(**id, "reviewer-a", decision).await.is_err());
            let second = self
                .decide(
                    **id,
                    "reviewer-b",
                    deepref_domain::ScreeningDecision::Exclude,
                )
                .await
                .unwrap();
            assert_eq!(second.revision, 0);
            assert_eq!(second.title_abstract_status, "unscreened");
        }
    }
    async fn status(&self) -> String {
        sqlx::query_scalar("SELECT status FROM ai_screening_cohorts WHERE id=$1")
            .bind(self.cohort)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn blinded_fixed_audit_finalizes_and_reopens_through_normal_events() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, true).await;
    let before = get_prisma_projection(&f.pool, f.project)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.ai_quarantined.get(), 40);
    assert_eq!(before.automation_excluded.get(), 0);
    screen_report(
        &f.pool,
        ScreenReportCommand {
            project_id: f.project.into(),
            report_id: f.human[0].into(),
            stage: ScreeningStage::FullText,
            decision: deepref_domain::ScreeningDecision::Include,
            exclusion_reason_id: None,
            protocol_version_id: f.protocol.into(),
            expected_revision: f.revision(f.human[0]).await,
            notes: None,
            actor: actor("reviewer-initial"),
        },
    )
    .await
    .unwrap();
    let sample = f.draw().await;
    assert!(
        draw_ai_first_audit(&f.pool, f.project, f.cohort, None, &actor("owner"))
            .await
            .is_err()
    );
    assert!(
        evaluate_ai_first_audit(&f.pool, f.project, f.cohort, &actor("owner"))
            .await
            .is_err()
    );
    let queue = get_screening_queue(
        &f.pool,
        GetScreeningQueueQuery {
            reviewer_id: Some("reviewer-a".into()),
            project_id: f.project.into(),
            status: ScreeningQueueStatus::Unscreened,
            search: None,
            sort: ScreeningQueueSort::CreatedAscending,
            cursor: None,
            limit: 100,
        },
    )
    .await
    .unwrap();
    assert_eq!(queue.items.len(), sample.len() + 20);
    assert!(
        queue
            .items
            .iter()
            .all(|r| r.title_abstract_status == "unscreened")
    );
    assert!(queue.items.iter().all(|r| r.revision == 0
        && r.full_text_status == "not_required"
        && r.final_status == "unscreened"));
    for id in f.x.iter().chain(f.human.iter()) {
        let state = get_agent_screening_state(&f.pool, f.project, *id)
            .await
            .unwrap();
        assert_eq!(state.revision, 0);
        assert_eq!(state.title_abstract_status, "unscreened");
        assert!(state.last_event_id.is_none());
        assert!(
            get_screening_history(&f.pool, f.project, *id)
                .await
                .unwrap()
                .items
                .is_empty()
        );
    }
    assert!(
        load_audit_export_rows(&f.pool, f.project, 10000)
            .await
            .unwrap()
            .iter()
            .all(|r| r.event_type != "screening")
    );
    assert!(
        f.decide(
            sample[0],
            "reviewer-initial",
            deepref_domain::ScreeningDecision::Exclude
        )
        .await
        .is_err()
    );
    assert!(
        f.decide(
            sample[0],
            "owner",
            deepref_domain::ScreeningDecision::Exclude
        )
        .await
        .is_err()
    );
    let mut filtered = GetScreeningQueueQuery {
        reviewer_id: Some("reviewer-a".into()),
        project_id: f.project.into(),
        status: ScreeningQueueStatus::Include,
        search: None,
        sort: ScreeningQueueSort::CreatedAscending,
        cursor: None,
        limit: 100,
    };
    assert!(
        get_screening_queue(&f.pool, filtered.clone())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    filtered.status = ScreeningQueueStatus::All;
    assert_eq!(
        get_screening_queue(&f.pool, filtered.clone())
            .await
            .unwrap()
            .total,
        queue.total
    );
    filtered.reviewer_id = Some("reviewer-initial".into());
    assert!(
        get_screening_queue(&f.pool, filtered)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let order: Vec<Uuid>=sqlx::query_scalar("SELECT report_id FROM ai_screening_cohort_members WHERE cohort_id=$1 AND audit_order IS NOT NULL ORDER BY audit_order")
        .bind(f.cohort).fetch_all(&f.pool).await.unwrap();
    assert_eq!(
        queue.items.iter().map(|r| r.report_id).collect::<Vec<_>>(),
        order,
        "default queue must use frozen mixed order"
    );
    let mut page_query = GetScreeningQueueQuery {
        reviewer_id: Some("reviewer-a".into()),
        project_id: f.project.into(),
        status: ScreeningQueueStatus::Unscreened,
        search: None,
        sort: ScreeningQueueSort::CreatedAscending,
        cursor: None,
        limit: 7,
    };
    let first = get_screening_queue(&f.pool, page_query.clone())
        .await
        .unwrap();
    page_query.cursor = first.next_cursor;
    let second = get_screening_queue(&f.pool, page_query).await.unwrap();
    assert_eq!(second.items[0].report_id, order[7]);
    let opinion = Uuid::new_v4();
    sqlx::query("INSERT INTO ai_reviewer_decisions(id,project_id,report_id,stage,decision,rationale,evidence,source,resolved_at,resolution)
      VALUES($1,$2,$3,'title_abstract','include','prior control opinion','[]','ai',now(),'kept_human')")
        .bind(opinion).bind(f.project).bind(f.human[0]).execute(&f.pool).await.unwrap();
    assert!(
        list_reviewer_decisions(&f.pool, f.project, None, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    let prior_proposal = Uuid::new_v4();
    let model_run: Uuid = sqlx::query_scalar(
        "SELECT ai_run_id FROM ai_screening_dispositions WHERE cohort_id=$1 LIMIT 1",
    )
    .bind(f.cohort)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO ai_proposals(id,project_id,ai_run_id,proposal_type,payload,status,entity_type,operation,model_run_id,authority_tier,task_kind,protocol_version_id,target_report_id)
      VALUES($1,$2,$3,'screening_suggestion','{\"stage\":\"full_text\"}','pending','screening_report','screening_suggestion',$3,'scientific_conclusion','full_text_screening',$4,$5)")
        .bind(prior_proposal).bind(f.project).bind(model_run).bind(f.protocol).bind(f.human[0])
        .execute(&f.pool).await.unwrap();
    assert!(
        matches!(
            get_visible_ai_proposal(&f.pool, f.project, prior_proposal).await,
            Err(AiProposalError::NotFound)
        ),
        "a prior full-text opinion must not identify an audit control"
    );
    assert!(
        list_ai_proposals(
            &f.pool,
            f.project,
            AiProposalFilters {
                status: None,
                task_kind: None,
                target_report_id: None,
                target_record_id: None,
                candidate_report_id: None,
                target_study_id: None,
            },
            None,
            100
        )
        .await
        .unwrap()
        .is_empty()
    );
    let mut entry = NewActivity::new(
        f.project,
        "assistant",
        "Earlier reviewer",
        actor("reviewer-initial"),
        "title_abstract_screening",
        "screening_decision",
        "Prior control decision",
    );
    entry.affected = serde_json::json!([{"type":"report","id":f.human[0],"label":"Prior control"}]);
    let activity = record_activity(&f.pool, &entry).await.unwrap();
    assert!(
        list_activity(&f.pool, f.project, ActivityFilters::default(), None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(get_activity(&f.pool, f.project, activity).await.is_err());
    assert!(
        load_audit_export_rows(&f.pool, f.project, 10000)
            .await
            .unwrap()
            .is_empty()
    );
    f.label_all(&sample, false).await;
    let result = evaluate_ai_first_audit(&f.pool, f.project, f.cohort, &actor("owner"))
        .await
        .unwrap();
    assert!(result.passed);
    assert_eq!(
        result.observed_relevant, 0,
        "positive controls must never enter k"
    );
    for id in &f.human {
        assert_eq!(
            get_agent_screening_state(&f.pool, f.project, *id)
                .await
                .unwrap()
                .title_abstract_status,
            "include"
        );
        assert_eq!(
            get_screening_history(&f.pool, f.project, *id)
                .await
                .unwrap()
                .items
                .len(),
            if *id == f.human[0] { 2 } else { 1 },
            "controls never write canonical events"
        );
    }

    assert!(
        evaluate_ai_first_audit(&f.pool, f.project, f.cohort, &actor("owner"))
            .await
            .is_err()
    );
    assert!(
        finalize_ai_first_cohort(&f.pool, f.project, f.cohort, false, &actor("owner"))
            .await
            .is_err()
    );
    let count = finalize_ai_first_cohort(&f.pool, f.project, f.cohort, true, &actor("owner"))
        .await
        .unwrap();
    assert_eq!(count, 40 - sample.len() as u32);
    let p = get_prisma_projection(&f.pool, f.project)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p.ai_quarantined.get(), 0);
    assert_eq!(p.automation_excluded.get(), u64::from(count));
    let export = load_audit_export_rows(&f.pool, f.project, 10000)
        .await
        .unwrap();
    assert!(
        export
            .iter()
            .any(|e| e.event_type == "ai_first_reference_label")
    );
    let excluded:Uuid=sqlx::query_scalar("SELECT report_id FROM ai_screening_dispositions WHERE cohort_id=$1 AND finalized_at IS NOT NULL LIMIT 1")
        .bind(f.cohort).fetch_one(&f.pool).await.unwrap();
    f.decide(
        excluded,
        "owner",
        deepref_domain::ScreeningDecision::Include,
    )
    .await
    .unwrap();
    assert_eq!(f.status().await, "invalidated");
    let p = get_prisma_projection(&f.pool, f.project)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p.automation_excluded.get(), 0);
    assert!(
        get_ai_first_overview(&f.pool, f.project)
            .await
            .unwrap()
            .suspended_reason
            .is_some()
    );
}

#[tokio::test]
async fn audit_failure_recovers_quarantine_and_cannot_redraw() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, true).await;
    let sample = f.draw().await;
    f.label_all(&sample, true).await;
    let result = evaluate_ai_first_audit(&f.pool, f.project, f.cohort, &actor("owner"))
        .await
        .unwrap();
    assert!(!result.passed);
    assert_eq!(f.status().await, "failed");
    assert!(
        start_ai_first_cohort(&f.pool, f.project, 95, true, &actor("owner"))
            .await
            .is_err(),
        "a failed frozen frame must return to humans, not be redrawn"
    );
    assert!(
        draw_ai_first_audit(&f.pool, f.project, f.cohort, None, &actor("owner"))
            .await
            .is_err()
    );
    assert_eq!(
        get_prisma_projection(&f.pool, f.project)
            .await
            .unwrap()
            .unwrap()
            .ai_quarantined
            .get(),
        0
    );
    assert!(
        get_ai_first_overview(&f.pool, f.project)
            .await
            .unwrap()
            .suspended_reason
            .is_some()
    );
}

#[tokio::test]
async fn routing_ceiling_cannot_finalize_even_passing_reference() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, false).await;
    let sample = f.draw().await;
    f.label_all(&sample, false).await;
    assert!(
        evaluate_ai_first_audit(&f.pool, f.project, f.cohort, &actor("owner"))
            .await
            .unwrap()
            .passed
    );
    assert!(
        finalize_ai_first_cohort(&f.pool, f.project, f.cohort, true, &actor("owner"))
            .await
            .is_err()
    );
    recover_ai_first_cohort(&f.pool, f.project, f.cohort, &actor("owner"))
        .await
        .unwrap();
    assert_eq!(f.status().await, "invalidated");
}

#[tokio::test]
async fn undo_reference_or_deleted_member_invalidates_frozen_design() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool.clone(), true).await;
    let sample = f.draw().await;
    assert!(
        undo_screening(
            &pool,
            UndoScreeningCommand {
                project_id: f.project.into(),
                report_id: f.human[0].into(),
                stage: ScreeningStage::TitleAbstract,
                protocol_version_id: f.protocol.into(),
                expected_revision: 1,
                notes: None,
                actor: actor("owner"),
            }
        )
        .await
        .is_err(),
        "blind audit edits must require recovery"
    );
    f.label_all(&sample, false).await;
    evaluate_ai_first_audit(&pool, f.project, f.cohort, &actor("owner"))
        .await
        .unwrap();
    undo_screening(
        &pool,
        UndoScreeningCommand {
            project_id: f.project.into(),
            report_id: f.human[0].into(),
            stage: ScreeningStage::TitleAbstract,
            protocol_version_id: f.protocol.into(),
            expected_revision: 1,
            notes: None,
            actor: actor("owner"),
        },
    )
    .await
    .unwrap();
    assert_eq!(f.status().await, "invalidated");
    let f = Fixture::new(pool, true).await;
    let sample = f.draw().await;
    f.label_all(&sample, false).await;
    sqlx::query("DELETE FROM project_reports WHERE project_id=$1 AND report_id=$2")
        .bind(f.project)
        .bind(sample[0])
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        evaluate_ai_first_audit(&f.pool, f.project, f.cohort, &actor("owner"))
            .await
            .is_err()
    );
    assert_eq!(f.status().await, "invalidated");
}

#[tokio::test]
async fn exposed_labels_and_edited_sources_refuse_and_release_records() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool.clone(), true).await;
    let sample = f.draw().await;
    sqlx::query("INSERT INTO ai_opinion_exposures(project_id,report_id,stage,exposure_source,exposure_possible_at)
       VALUES($1,$2,'title_abstract','workflow_run_output',clock_timestamp())").bind(f.project).bind(sample[0]).execute(&pool).await.unwrap();
    f.decide(
        sample[0],
        "reviewer-a",
        deepref_domain::ScreeningDecision::Exclude,
    )
    .await
    .unwrap();
    assert_eq!(f.status().await, "invalidated");
    let f = Fixture::new(pool, true).await;
    let sample = f.draw().await;
    f.label_all(&sample, false).await;
    sqlx::query("UPDATE reports SET abstract_text='Changed evidence' WHERE id=$1")
        .bind(f.x[0])
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        evaluate_ai_first_audit(&f.pool, f.project, f.cohort, &actor("owner"))
            .await
            .is_err()
    );
    assert_eq!(f.status().await, "invalidated");
}

#[tokio::test]
async fn protocol_amendment_reopens_finalized_automation_and_preserves_alpha_budget() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, true).await;
    let sample = f.draw().await;
    f.label_all(&sample, false).await;
    evaluate_ai_first_audit(&f.pool, f.project, f.cohort, &actor("owner"))
        .await
        .unwrap();
    finalize_ai_first_cohort(&f.pool, f.project, f.cohort, true, &actor("owner"))
        .await
        .unwrap();
    Fixture::publish(&f.pool, f.project).await;
    assert_eq!(f.status().await, "invalidated");
    assert_eq!(
        get_prisma_projection(&f.pool, f.project)
            .await
            .unwrap()
            .unwrap()
            .automation_excluded
            .get(),
        0
    );
    let ordinal: i32 = sqlx::query_scalar(
        "SELECT next_audit_ordinal FROM project_ai_screening_authority WHERE project_id=$1",
    )
    .bind(f.project)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(ordinal, 2);
}

#[tokio::test]
async fn audit_draw_serializes_and_last_allocation_is_never_reset() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, true).await;
    sqlx::query(
        "UPDATE project_ai_screening_authority SET next_audit_ordinal=25 WHERE project_id=$1",
    )
    .bind(f.project)
    .execute(&f.pool)
    .await
    .unwrap();
    close_ai_first_cohort(&f.pool, f.project, f.cohort, &actor("owner"))
        .await
        .unwrap();
    let owner_a = actor("owner-a");
    let owner_b = actor("owner-b");
    let (a, b) = tokio::join!(
        draw_ai_first_audit(&f.pool, f.project, f.cohort, None, &owner_a),
        draw_ai_first_audit(&f.pool, f.project, f.cohort, None, &owner_b)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(
        a.ok().or_else(|| b.ok()),
        Some(39),
        "the exact zero-tail minimum at the last allocation is 39"
    );
    let overview = get_ai_first_overview(&f.pool, f.project).await.unwrap();
    assert_eq!(overview.cohorts[0].alpha_billionths, Some(1));
    recover_ai_first_cohort(&f.pool, f.project, f.cohort, &actor("owner"))
        .await
        .unwrap();
    let ordinal: i32 = sqlx::query_scalar(
        "SELECT next_audit_ordinal FROM project_ai_screening_authority WHERE project_id=$1",
    )
    .bind(f.project)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(ordinal, 26);
    assert_eq!(
        get_ai_first_overview(&f.pool, f.project)
            .await
            .unwrap()
            .ceiling,
        "off"
    );
}

#[tokio::test]
async fn exhausted_review_wide_allocation_refuses_without_changing_evidence() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, true).await;
    sqlx::query(
        "UPDATE project_ai_screening_authority SET next_audit_ordinal=26 WHERE project_id=$1",
    )
    .bind(f.project)
    .execute(&f.pool)
    .await
    .unwrap();
    close_ai_first_cohort(&f.pool, f.project, f.cohort, &actor("owner"))
        .await
        .unwrap();
    assert!(
        draw_ai_first_audit(&f.pool, f.project, f.cohort, None, &actor("owner"))
            .await
            .is_err()
    );
    assert_eq!(f.status().await, "closed");
    let n: i32 = sqlx::query_scalar(
        "SELECT next_audit_ordinal FROM project_ai_screening_authority WHERE project_id=$1",
    )
    .bind(f.project)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(n, 26);
    assert_eq!(
        get_prisma_projection(&f.pool, f.project)
            .await
            .unwrap()
            .unwrap()
            .ai_quarantined
            .get(),
        40
    );
}

#[tokio::test]
async fn a_new_positive_control_rescues_and_invalidates_before_the_statistical_look() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, true).await;
    f.decide(
        f.human[0],
        "reviewer-initial",
        deepref_domain::ScreeningDecision::Exclude,
    )
    .await
    .unwrap();
    let sample = f.draw().await;
    f.label_all(&sample, false).await;
    assert!(
        evaluate_ai_first_audit(&f.pool, f.project, f.cohort, &actor("owner"))
            .await
            .is_err()
    );
    assert_eq!(f.status().await, "invalidated");
    assert_eq!(
        get_agent_screening_state(&f.pool, f.project, f.human[0])
            .await
            .unwrap()
            .title_abstract_status,
        "include"
    );
    let no_look:bool=sqlx::query_scalar("SELECT result IS NULL AND evaluated_at IS NULL AND reference_relevant=19 FROM ai_screening_cohorts WHERE id=$1")
        .bind(f.cohort).fetch_one(&f.pool).await.unwrap();
    assert!(no_look, "never relabel R retrospectively or report a pass");
    assert_eq!(
        get_prisma_projection(&f.pool, f.project)
            .await
            .unwrap()
            .unwrap()
            .ai_quarantined
            .get(),
        0
    );
    assert!(
        finalize_ai_first_cohort(&f.pool, f.project, f.cohort, true, &actor("owner"))
            .await
            .is_err()
    );
}
