#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Operator decisions, conservative identity matching, rolling compatibility and provenance.

use deepref_app::{
    identity::{Deactivation, deactivate_user},
    list_legacy_actor_decisions, map_legacy_actor, record_legacy_actor_non_identity,
};
use deepref_domain::{ExternalUserId, UserId};
use sqlx::{AssertSqlSafe, PgPool};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 6).await
}

async fn create_user(pool: &PgPool, label: &str) -> (UserId, String) {
    let id = UserId::new(Uuid::new_v4());
    let external_id = format!("continuity-{label}-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO users(id,workos_user_id,primary_email,status) \
         VALUES($1,$2,$3,'active')",
    )
    .bind(id.as_uuid())
    .bind(&external_id)
    .bind(format!("{label}-{}@example.test", Uuid::new_v4()))
    .execute(pool)
    .await
    .unwrap();
    (id, external_id)
}

struct Fixture {
    pool: PgPool,
    project_id: Uuid,
    report_id: Uuid,
    protocol_id: Uuid,
    cohort_id: Uuid,
    candidate: UserId,
    owner: UserId,
    operator: UserId,
}

impl Fixture {
    async fn new(pool: PgPool, approved_by: impl Into<String>) -> Self {
        let (candidate, _) = create_user(&pool, "candidate").await;
        let (owner, _) = create_user(&pool, "owner").await;
        let (operator, _) = create_user(&pool, "operator").await;
        let project_id = Uuid::new_v4();
        sqlx::query("INSERT INTO projects(id,name) VALUES($1,'Legacy actor continuity')")
            .bind(project_id)
            .execute(&pool)
            .await
            .unwrap();
        let protocol_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO protocol_versions(id,project_id,version,name,status,criteria) \
             VALUES($1,$2,1,'Continuity protocol','published','[]')",
        )
        .bind(protocol_id)
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
        let report_id = Uuid::new_v4();
        sqlx::query("INSERT INTO reports(id,title) VALUES($1,'Continuity report')")
            .bind(report_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
            .bind(project_id)
            .bind(report_id)
            .execute(&pool)
            .await
            .unwrap();
        let cohort_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO ai_screening_cohorts \
             (id,project_id,protocol_version_id,semantic_bundle_hash,semantic_identity,policy_version,status,target_percent,approved_by) \
             VALUES($1,$2,$3,$4,'{}',1,'auditing',95,$5)",
        )
        .bind(cohort_id)
        .bind(project_id)
        .bind(protocol_id)
        .bind("b".repeat(64))
        .bind(approved_by.into())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO ai_screening_cohort_members \
             (project_id,cohort_id,report_id,evaluated_revision,sampled) \
             VALUES($1,$2,$3,0,true)",
        )
        .bind(project_id)
        .bind(cohort_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO scientific_assignments(project_id,user_id,role,assigned_by) \
             VALUES($1,$2,'fresh_ai_auditor',$3)",
        )
        .bind(project_id)
        .bind(candidate.as_uuid())
        .bind(owner.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
        Self {
            pool,
            project_id,
            report_id,
            protocol_id,
            cohort_id,
            candidate,
            owner,
            operator,
        }
    }

    async fn fresh_v2(&self, user_id: UserId) -> bool {
        sqlx::query_scalar("SELECT ai_first_fresh_auditor_v2($1,$2)")
            .bind(self.cohort_id)
            .bind(user_id.as_uuid())
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn assign_fresh_auditor(&self, user_id: UserId) {
        sqlx::query(
            "INSERT INTO scientific_assignments(project_id,user_id,role,assigned_by) \
             VALUES($1,$2,'fresh_ai_auditor',$3)",
        )
        .bind(self.project_id)
        .bind(user_id.as_uuid())
        .bind(self.owner.as_uuid())
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn insert_event(&self, actor_id: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO screening_events \
             (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id) \
             VALUES($1,$2,$3,'title_abstract','include',$4,'user',$5)",
        )
        .bind(id)
        .bind(self.project_id)
        .bind(self.report_id)
        .bind(self.protocol_id)
        .bind(actor_id)
        .execute(&self.pool)
        .await
        .unwrap();
        id
    }

    async fn insert_audience_exposure(&self, actor_id: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO ai_opinion_exposures \
             (id,project_id,report_id,stage,exposure_source,audience_actor_kind,audience_actor_id,exposure_possible_at) \
             VALUES($1,$2,$3,'title_abstract','workflow_run_output','user',$4,now())",
        )
        .bind(id)
        .bind(self.project_id)
        .bind(self.report_id)
        .bind(actor_id)
        .execute(&self.pool)
        .await
        .unwrap();
        id
    }

    async fn insert_label(&self, actor_id: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO ai_screening_audit_labels \
             (id,project_id,cohort_id,report_id,actor_id,decision) \
             VALUES($1,$2,$3,$4,$5,'include')",
        )
        .bind(id)
        .bind(self.project_id)
        .bind(self.cohort_id)
        .bind(self.report_id)
        .bind(actor_id)
        .execute(&self.pool)
        .await
        .unwrap();
        id
    }
}

#[tokio::test]
async fn mapped_exposed_legacy_label_makes_candidate_ineligible_and_is_audited() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-owner").await;
    sqlx::query("UPDATE ai_screening_cohorts SET approved_by=$2 WHERE id=$1")
        .bind(f.cohort_id)
        .bind(f.owner.as_uuid().to_string())
        .execute(&f.pool)
        .await
        .unwrap();
    f.assign_fresh_auditor(f.owner).await;
    let label = "legacy-reviewer-42";
    f.insert_audience_exposure(label).await;
    let (unexposed_user, _) = create_user(&f.pool, "unexposed").await;
    f.assign_fresh_auditor(unexposed_user).await;

    let before = f.fresh_v2(f.candidate).await;
    assert!(
        !before,
        "unmapped exposure conservatively excludes every user"
    );
    let decision = map_legacy_actor(
        &f.pool,
        "user",
        label,
        f.candidate,
        f.operator,
        "verified from signed review record",
    )
    .await
    .unwrap();
    assert_eq!(decision.user_id, f.candidate);
    assert!(
        !f.fresh_v2(f.candidate).await,
        "mapped exposure excludes its user"
    );
    assert!(
        f.fresh_v2(unexposed_user).await,
        "mapping to another user is not exposure"
    );

    let audit: (String, Uuid, Uuid, serde_json::Value) = sqlx::query_as(
        "SELECT event_type,principal_user_id,target_user_id,metadata \
         FROM admin_audit_events WHERE event_type='legacy_actor.mapped' \
         AND target_user_id=$1 ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(f.candidate.as_uuid())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(audit.0, "legacy_actor.mapped");
    assert_eq!(audit.1, f.operator.as_uuid());
    assert_eq!(audit.2, f.candidate.as_uuid());
    assert_eq!(audit.3["legacy_actor_id"], label);

    map_legacy_actor(
        &f.pool,
        "user",
        label,
        f.candidate,
        f.operator,
        "a later duplicate confirmation",
    )
    .await
    .unwrap();
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_audit_events WHERE event_type='legacy_actor.mapped' AND target_user_id=$1",
    )
    .bind(f.candidate.as_uuid())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        audit_count, 1,
        "idempotent commands do not duplicate the audit"
    );
}

#[tokio::test]
async fn non_identity_decision_lifts_only_that_users_unknown_label_exclusion() {
    let Some(pool) = database().await else { return };
    let label = "reviewer-from-an-old-tab";
    let f = Fixture::new(pool, label).await;
    f.assign_fresh_auditor(f.owner).await;
    let active: bool = f.fresh_v2(f.candidate).await;
    assert!(!active, "the unmapped approval label blocks all candidates");

    let decision = record_legacy_actor_non_identity(
        &f.pool,
        "user",
        label,
        f.candidate,
        f.operator,
        "confirmed against reviewer roster",
    )
    .await
    .unwrap();
    assert_eq!(decision.user_id, f.candidate);
    assert!(f.fresh_v2(f.candidate).await);
    assert!(
        !f.fresh_v2(f.owner).await,
        "other users remain conservatively exposed"
    );

    let decisions = list_legacy_actor_decisions(&f.pool).await.unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].decision,
        deepref_app::LegacyActorDecisionKind::NonIdentity
    );
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_audit_events WHERE event_type='legacy_actor.non_identity_recorded' \
         AND target_user_id=$1 AND principal_user_id=$2",
    )
    .bind(f.candidate.as_uuid())
    .bind(f.operator.as_uuid())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(audited, 1);
}

#[tokio::test]
async fn mapped_labels_for_someone_else_do_not_exclude_the_candidate() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "legacy-reviewer").await;
    let (other_user, _) = create_user(&f.pool, "mapped-other").await;
    f.assign_fresh_auditor(other_user).await;
    map_legacy_actor(
        &f.pool,
        "user",
        "legacy-reviewer",
        other_user,
        f.operator,
        "operator verified distinct reviewers",
    )
    .await
    .unwrap();
    assert!(f.fresh_v2(f.candidate).await);
    assert!(!f.fresh_v2(other_user).await);
}

#[tokio::test]
async fn n_minus_one_approval_identity_comparison_stays_text_based() {
    let Some(pool) = database().await else { return };
    let (candidate, _) = create_user(&pool, "candidate").await;
    let f = Fixture::new(pool, "legacy-approved-by").await;
    let approved_match: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind("legacy-approved-by")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(!approved_match, "0055 approved_by compares the legacy text");
    let fresh: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind(candidate.as_uuid().to_string())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(fresh);
}

#[tokio::test]
async fn n_minus_one_screening_event_identity_comparison_stays_text_based() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-approver").await;
    f.insert_event("legacy-screening-actor").await;
    let screening_match: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind("legacy-screening-actor")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        !screening_match,
        "0055 screening actor comparison is unchanged"
    );
}

#[tokio::test]
async fn n_minus_one_exposure_audience_identity_comparison_stays_text_based() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-approver").await;
    f.insert_audience_exposure("legacy-exposure-audience").await;
    let exposure_match: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind("legacy-exposure-audience")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(!exposure_match, "0055 audience comparison is unchanged");
}

#[tokio::test]
async fn the_n_minus_one_queue_still_matches_legacy_audit_labels() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-approver").await;
    f.insert_label("legacy-auditor").await;
    let same_label: bool = sqlx::query_scalar("SELECT ai_first_queue_visible($1,$2,$3)")
        .bind(f.project_id)
        .bind(f.report_id)
        .bind("legacy-auditor")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let another_label: bool = sqlx::query_scalar("SELECT ai_first_queue_visible($1,$2,$3)")
        .bind(f.project_id)
        .bind(f.report_id)
        .bind("another-legacy-auditor")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        !same_label,
        "0055 queue comparison still hides an existing label"
    );
    assert!(another_label, "the next N-1 reviewer remains eligible");
}

#[tokio::test]
async fn audit_label_precheck_matches_canonical_and_operator_mapped_users() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-approver").await;
    let label = "legacy-audience-label";
    map_legacy_actor(
        &f.pool,
        "user",
        label,
        f.candidate,
        f.operator,
        "verified reviewer record",
    )
    .await
    .unwrap();
    let mapped: bool = sqlx::query_scalar("SELECT legacy_human_actor_matches_user('user',$1,$2)")
        .bind(label)
        .bind(f.candidate.as_uuid())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let canonical: bool =
        sqlx::query_scalar("SELECT legacy_human_actor_matches_user('user',$1,$2)")
            .bind(f.candidate.as_uuid().to_string())
            .bind(f.candidate.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(mapped);
    assert!(canonical);
}

#[tokio::test]
async fn frozen_evidence_comparison_respects_mappings_and_non_identity_records() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-approver").await;
    let label = "frozen-audience-label";
    map_legacy_actor(
        &f.pool,
        "user",
        label,
        f.candidate,
        f.operator,
        "review ledger",
    )
    .await
    .unwrap();
    let mapped_match: bool =
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,'user',$2)")
            .bind(label)
            .bind(f.candidate.as_uuid().to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let other_match: bool =
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,'user',$2)")
            .bind(label)
            .bind(f.owner.as_uuid().to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(mapped_match);
    assert!(!other_match, "a mapping to someone else is not a match");

    let unknown = "unmapped-frozen-label";
    let conservative_match: bool =
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,'user',$2)")
            .bind(unknown)
            .bind(f.candidate.as_uuid().to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(conservative_match);
    record_legacy_actor_non_identity(
        &f.pool,
        "user",
        unknown,
        f.candidate,
        f.operator,
        "confirmed reviewer roster",
    )
    .await
    .unwrap();
    let denied_match: bool =
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,'user',$2)")
            .bind(unknown)
            .bind(f.candidate.as_uuid().to_string())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(!denied_match);
}

#[test]
fn the_0054_queue_comparison_is_enumerated_and_superseded_by_0055() {
    let original = include_str!("../migrations/0054_ai_first_cohorts.sql");
    assert!(original.contains("l.actor_id=actor"));
    let replacement = include_str!("../migrations/0055_ai_first_blind_controls.sql");
    assert!(replacement.contains("l.actor_id=actor"));
}

#[tokio::test]
async fn actor_history_is_byte_identical_across_0106_and_after_new_writes() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "historical-approved-by").await;
    let screening_id = f.insert_event("legacy-screening-label").await;
    let exposure_id = f.insert_audience_exposure("legacy-exposure-label").await;
    let label_id = f.insert_label("legacy-audit-label").await;
    let before = historical_rows(&f, screening_id, exposure_id, label_id).await;

    // Reapply the real 0106 migration after removing only its functions and
    // migration ledger row. The historical fixture rows remain in place.
    for (name, signature) in [
        ("ai_first_queue_visible_v2", "uuid,uuid,uuid"),
        ("ai_first_fresh_auditor_v2", "uuid,uuid"),
        ("same_human_actor_identity", "text,text,text,text"),
        ("legacy_human_actor_is_unmapped", "text,text"),
        ("legacy_human_actor_user_id", "text,text"),
        ("legacy_human_actor_matches_user", "text,text,uuid"),
    ] {
        sqlx::query(AssertSqlSafe(format!("DROP FUNCTION {name}({signature})")))
            .execute(&f.pool)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version=106")
        .execute(&f.pool)
        .await
        .unwrap();
    deepref_app::migrate(&f.pool).await.unwrap();

    let after_migration = historical_rows(&f, screening_id, exposure_id, label_id).await;
    assert_eq!(
        after_migration, before,
        "0106 leaves every history row unchanged"
    );

    let new_screening_id = f.insert_event(&f.candidate.as_uuid().to_string()).await;
    let new_exposure_id = f
        .insert_audience_exposure(&f.candidate.as_uuid().to_string())
        .await;
    let new_label_id = f.insert_label(&f.candidate.as_uuid().to_string()).await;
    assert_ne!(new_screening_id, screening_id);
    assert_ne!(new_exposure_id, exposure_id);
    assert_ne!(new_label_id, label_id);
    let after_new_write = historical_rows(&f, screening_id, exposure_id, label_id).await;
    assert_eq!(after_new_write, before, "new writes do not rewrite history");
}

async fn historical_rows(
    fixture: &Fixture,
    screening_id: Uuid,
    exposure_id: Uuid,
    label_id: Uuid,
) -> (String, String, String, String) {
    let screening =
        sqlx::query_scalar("SELECT row_to_json(s)::text FROM screening_events s WHERE id=$1")
            .bind(screening_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    let exposure =
        sqlx::query_scalar("SELECT row_to_json(e)::text FROM ai_opinion_exposures e WHERE id=$1")
            .bind(exposure_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    let label = sqlx::query_scalar(
        "SELECT row_to_json(l)::text FROM ai_screening_audit_labels l WHERE id=$1",
    )
    .bind(label_id)
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    let approval =
        sqlx::query_scalar("SELECT row_to_json(c)::text FROM ai_screening_cohorts c WHERE id=$1")
            .bind(fixture.cohort_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    (screening, exposure, label, approval)
}

#[tokio::test]
async fn deactivation_preserves_historical_actor_and_exposure() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-owner").await;
    sqlx::query("UPDATE ai_screening_cohorts SET approved_by=$2 WHERE id=$1")
        .bind(f.cohort_id)
        .bind(f.owner.as_uuid().to_string())
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, external_id) = create_user(&f.pool, "deactivated-candidate").await;
    let deactivated_user: UserId =
        sqlx::query_scalar("SELECT id FROM users WHERE workos_user_id=$1")
            .bind(&external_id)
            .fetch_one(&f.pool)
            .await
            .map(UserId::new)
            .unwrap();
    sqlx::query(
        "INSERT INTO scientific_assignments(project_id,user_id,role,assigned_by) \
         VALUES($1,$2,'fresh_ai_auditor',$3)",
    )
    .bind(f.project_id)
    .bind(deactivated_user.as_uuid())
    .bind(f.owner.as_uuid())
    .execute(&f.pool)
    .await
    .unwrap();
    f.insert_event(&deactivated_user.as_uuid().to_string())
        .await;
    let result = deactivate_user(
        &f.pool,
        &ExternalUserId::parse(&external_id).unwrap(),
        "actor-continuity-deactivation",
    )
    .await
    .unwrap();
    assert_eq!(result, Deactivation::Deactivated(deactivated_user));
    let stored: String = sqlx::query_scalar(
        "SELECT actor_id FROM screening_events WHERE project_id=$1 AND actor_id=$2",
    )
    .bind(f.project_id)
    .bind(deactivated_user.as_uuid().to_string())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(stored, deactivated_user.as_uuid().to_string());
    let counted = f.fresh_v2(deactivated_user).await;
    assert!(!counted, "the deactivated person's provenance still counts");
    let deactivated: bool =
        sqlx::query_scalar("SELECT status='deactivated' FROM users WHERE id=$1")
            .bind(deactivated_user.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(deactivated);
}
