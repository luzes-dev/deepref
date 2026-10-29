#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Operator decisions, conservative identity matching, rolling compatibility and provenance.

use deepref_app::{
    identity::{Deactivation, deactivate_user},
    list_legacy_actor_decisions, map_legacy_actor, record_legacy_actor_non_identity,
};
use deepref_domain::{ExternalUserId, UserId};
use sqlx::PgPool;
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

    async fn fresh_v3(&self, user_id: UserId) -> bool {
        sqlx::query_scalar("SELECT ai_first_fresh_auditor_v3($1,$2,$3)")
            .bind(self.cohort_id)
            .bind(self.report_id)
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

    async fn set_approver(&self, user_id: UserId) {
        sqlx::query("UPDATE ai_screening_cohorts SET approved_by=$2,approved_by_actor_user_id=$3 WHERE id=$1")
            .bind(self.cohort_id)
            .bind(user_id.as_uuid().to_string())
            .bind(user_id.as_uuid())
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

    async fn insert_label_with_user(&self, actor_id: &str, actor_user_id: UserId) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO ai_screening_audit_labels \
             (id,project_id,cohort_id,report_id,actor_id,actor_user_id,decision) \
             VALUES($1,$2,$3,$4,$5,$6,'include')",
        )
        .bind(id)
        .bind(self.project_id)
        .bind(self.cohort_id)
        .bind(self.report_id)
        .bind(actor_id)
        .bind(actor_user_id.as_uuid())
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
    sqlx::query(
        "UPDATE ai_screening_cohorts SET approved_by=$2,approved_by_actor_user_id=$3 WHERE id=$1",
    )
    .bind(f.cohort_id)
    .bind(f.owner.as_uuid().to_string())
    .bind(f.owner.as_uuid())
    .execute(&f.pool)
    .await
    .unwrap();
    f.assign_fresh_auditor(f.owner).await;
    let label = "legacy-reviewer-42";
    f.insert_audience_exposure(label).await;
    let (unexposed_user, _) = create_user(&f.pool, "unexposed").await;
    f.assign_fresh_auditor(unexposed_user).await;

    let before = f.fresh_v3(f.candidate).await;
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
        !f.fresh_v3(f.candidate).await,
        "mapped exposure excludes its user"
    );
    assert!(
        f.fresh_v3(unexposed_user).await,
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
async fn non_identity_decision_does_not_prove_legacy_reviewer_distinctness() {
    let Some(pool) = database().await else { return };
    let label = "reviewer-from-an-old-tab";
    let f = Fixture::new(pool, label).await;
    f.assign_fresh_auditor(f.owner).await;
    let active: bool = f.fresh_v3(f.candidate).await;
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
    assert!(
        !f.fresh_v3(f.candidate).await,
        "a negative decision does not resolve the legacy label to another user"
    );
    assert!(
        !f.fresh_v3(f.owner).await,
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
async fn unresolved_labels_stay_potentially_identical_after_all_user_denials() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "legacy-approver").await;
    let labels = ["unknown-reviewer-a", "unknown-reviewer-b"];
    let users: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM users")
        .fetch_all(&f.pool)
        .await
        .unwrap();
    for label in labels {
        for user_id in &users {
            record_legacy_actor_non_identity(
                &f.pool,
                "user",
                label,
                UserId::new(*user_id),
                f.operator,
                "excluded from the complete current user roster",
            )
            .await
            .unwrap();
        }
    }

    let could_be_same: bool =
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,NULL,'user',$2,NULL)")
            .bind(labels[0])
            .bind(labels[1])
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(could_be_same, "unknown people can exist outside users");

    sqlx::query(
        "INSERT INTO ai_screening_audit_labels(project_id,cohort_id,report_id,actor_id,decision) \
         VALUES($1,$2,$3,$4,'include')",
    )
    .bind(f.project_id)
    .bind(f.cohort_id)
    .bind(f.report_id)
    .bind(labels[0])
    .execute(&f.pool)
    .await
    .unwrap();
    let second_insert = sqlx::query(
        "INSERT INTO ai_screening_audit_labels(project_id,cohort_id,report_id,actor_id,decision) \
         VALUES($1,$2,$3,$4,'exclude')",
    )
    .bind(f.project_id)
    .bind(f.cohort_id)
    .bind(f.report_id)
    .bind(labels[1])
    .execute(&f.pool)
    .await;
    assert!(
        second_insert.is_err(),
        "N-1 raw inserts cannot create two reviewer rows with unresolved identities"
    );

    let legacy_fresh: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind(labels[1])
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        !legacy_fresh,
        "the old signature must fail closed for unknown identities"
    );
    let v2_fresh: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor_v2($1,$2)")
        .bind(f.cohort_id)
        .bind(f.candidate.as_uuid())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(!v2_fresh, "the v2 signature remains fail closed");
    let reviewer_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_screening_audit_labels WHERE cohort_id=$1 AND report_id=$2",
    )
    .bind(f.cohort_id)
    .bind(f.report_id)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(reviewer_rows, 1);
}

#[tokio::test]
async fn n_minus_one_approver_upsert_clears_old_provenance() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "unused-approver").await;
    sqlx::query(
        "INSERT INTO project_ai_screening_authority \
         (project_id,ceiling,approved_by,approved_by_actor_user_id) \
         VALUES($1,'routing',$2,$3)",
    )
    .bind(f.project_id)
    .bind(f.candidate.as_uuid().to_string())
    .bind(f.candidate.as_uuid())
    .execute(&f.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO project_ai_screening_authority(project_id,ceiling,approved_by) \
         VALUES($1,'routing','legacy-approver-b') \
         ON CONFLICT(project_id) DO UPDATE SET approved_by=EXCLUDED.approved_by",
    )
    .bind(f.project_id)
    .execute(&f.pool)
    .await
    .unwrap();

    let (label, provenance): (String, Option<Uuid>) = sqlx::query_as(
        "SELECT approved_by,approved_by_actor_user_id \
         FROM project_ai_screening_authority WHERE project_id=$1",
    )
    .bind(f.project_id)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(label, "legacy-approver-b");
    assert_eq!(provenance, None);
    let still_may_be_previous_approver: bool =
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,$2,'user',$3,$4)")
            .bind(label)
            .bind(provenance)
            .bind(f.candidate.as_uuid().to_string())
            .bind(f.candidate.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(still_may_be_previous_approver);
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
    assert!(f.fresh_v3(f.candidate).await);
    assert!(!f.fresh_v3(other_user).await);
}

#[tokio::test]
async fn uuid_shaped_legacy_label_stays_unknown_until_operator_decision() {
    let Some(pool) = database().await else { return };
    let (uuid_label_owner, _) = create_user(&pool, "uuid-label-owner").await;
    let uuid_label = uuid_label_owner.as_uuid().to_string();
    let f = Fixture::new(pool, uuid_label.clone()).await;
    f.assign_fresh_auditor(uuid_label_owner).await;

    assert!(!f.fresh_v3(f.candidate).await);
    assert!(
        !f.fresh_v3(uuid_label_owner).await,
        "a UUID-shaped label has no implicit owner"
    );

    map_legacy_actor(
        &f.pool,
        "user",
        &uuid_label,
        f.candidate,
        f.operator,
        "operator matched the historical reviewer log",
    )
    .await
    .unwrap();
    assert!(
        !f.fresh_v3(f.candidate).await,
        "an explicit mapping counts as history"
    );
    assert!(
        f.fresh_v3(uuid_label_owner).await,
        "trusted provenance is distinct from a legacy label mapped to someone else"
    );

    let unresolved_uuid_label = Uuid::new_v4().to_string();
    let second = Fixture::new(f.pool.clone(), unresolved_uuid_label.clone()).await;
    assert!(!second.fresh_v3(second.candidate).await);
    record_legacy_actor_non_identity(
        &second.pool,
        "user",
        &unresolved_uuid_label,
        second.candidate,
        second.operator,
        "operator confirmed the UUID-shaped label belongs to someone else",
    )
    .await
    .unwrap();
    assert!(
        !second.fresh_v3(second.candidate).await,
        "a denial does not resolve an unknown label to a different user"
    );
}

#[tokio::test]
async fn trusted_provenance_precedes_a_conflicting_uuid_label_mapping() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "unused-approver").await;
    let bob = f.candidate;
    let bob_label = bob.as_uuid().to_string();
    let (alice, _) = create_user(&f.pool, "uuid-label-mapping-alice").await;
    let (charlie, _) = create_user(&f.pool, "trusted-charlie").await;
    f.assign_fresh_auditor(charlie).await;

    sqlx::query(
        "UPDATE ai_screening_cohorts \
         SET approved_by=$2,approved_by_actor_user_id=NULL WHERE id=$1",
    )
    .bind(f.cohort_id)
    .bind(&bob_label)
    .execute(&f.pool)
    .await
    .unwrap();
    map_legacy_actor(
        &f.pool,
        "user",
        &bob_label,
        alice,
        f.operator,
        "historical label matches a current user's UUID",
    )
    .await
    .unwrap();

    assert!(
        f.fresh_v3(bob).await,
        "a conflicting legacy mapping cannot refuse authenticated Bob"
    );
    assert!(
        f.fresh_v3(charlie).await,
        "the mapped historical Alice is distinct from authenticated Charlie"
    );

    let bob_may_be_alice: bool =
        sqlx::query_scalar("SELECT actor_identity_may_be_user('user',$1,$2,$3)")
            .bind(&bob_label)
            .bind(bob.as_uuid())
            .bind(alice.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let bob_may_be_bob: bool =
        sqlx::query_scalar("SELECT actor_identity_may_be_user('user',$1,$2,$3)")
            .bind(&bob_label)
            .bind(bob.as_uuid())
            .bind(bob.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(!bob_may_be_alice);
    assert!(bob_may_be_bob);

    let bob_and_charlie_same: bool =
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,$2,'user',$3,$4)")
            .bind(&bob_label)
            .bind(bob.as_uuid())
            .bind(charlie.as_uuid().to_string())
            .bind(charlie.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(!bob_and_charlie_same);

    f.insert_label_with_user(&bob_label, bob).await;
    f.insert_label_with_user(&charlie.as_uuid().to_string(), charlie)
        .await;
    let two_distinct_reviewers: bool =
        sqlx::query_scalar("SELECT ai_first_audit_has_two_distinct_reviewers($1,$2)")
            .bind(f.cohort_id)
            .bind(f.report_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(two_distinct_reviewers);

    let legacy_report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'Legacy UUID label report')")
        .bind(legacy_report_id)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(f.project_id)
        .bind(legacy_report_id)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ai_screening_cohort_members \
         (project_id,cohort_id,report_id,evaluated_revision,sampled) \
         VALUES($1,$2,$3,0,true)",
    )
    .bind(f.project_id)
    .bind(f.cohort_id)
    .bind(legacy_report_id)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ai_screening_audit_labels \
         (project_id,cohort_id,report_id,actor_id,decision) \
         VALUES($1,$2,$3,$4,'include')",
    )
    .bind(f.project_id)
    .bind(f.cohort_id)
    .bind(legacy_report_id)
    .bind(&bob_label)
    .execute(&f.pool)
    .await
    .unwrap();

    let (provenance, resolved_user, may_be_alice, may_be_bob): (
        Option<Uuid>,
        Option<Uuid>,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT l.actor_user_id, \
                resolved_human_actor_user_id('user',l.actor_id,l.actor_user_id), \
                actor_identity_may_be_user('user',l.actor_id,l.actor_user_id,$4), \
                actor_identity_may_be_user('user',l.actor_id,l.actor_user_id,$5) \
         FROM ai_screening_audit_labels l \
         WHERE l.cohort_id=$1 AND l.report_id=$2 AND l.actor_id=$3",
    )
    .bind(f.cohort_id)
    .bind(legacy_report_id)
    .bind(&bob_label)
    .bind(alice.as_uuid())
    .bind(bob.as_uuid())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(provenance, None);
    assert_eq!(resolved_user, Some(alice.as_uuid()));
    assert!(may_be_alice);
    assert!(!may_be_bob);
}

#[tokio::test]
async fn byte_distinct_case_and_whitespace_labels_are_resolved_independently() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "Reviewer-A ").await;
    f.insert_label("Reviewer-A ").await;
    assert!(!f.fresh_v3(f.candidate).await);

    map_legacy_actor(
        &f.pool,
        "user",
        "Reviewer-A ",
        f.owner,
        f.operator,
        "exact label including trailing space",
    )
    .await
    .unwrap();
    assert!(f.fresh_v3(f.candidate).await);
    let lower_label_mapping: Option<Uuid> =
        sqlx::query_scalar("SELECT legacy_human_actor_user_id('user','reviewer-a')")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(lower_label_mapping, None, "labels remain byte exact");

    map_legacy_actor(
        &f.pool,
        "user",
        "reviewer-a",
        f.candidate,
        f.operator,
        "exact lowercase label belongs to the other reviewer",
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ai_screening_audit_labels(project_id,cohort_id,report_id,actor_id,decision) \
         VALUES($1,$2,$3,'reviewer-a','include')",
    )
    .bind(f.project_id)
    .bind(f.cohort_id)
    .bind(f.report_id)
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(!f.fresh_v3(f.candidate).await);
}

#[tokio::test]
async fn an_unmapped_audit_label_hides_its_report_from_every_unresolved_user() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "temporary-owner-label").await;
    f.set_approver(f.owner).await;
    f.insert_label("reviewer-a").await;

    assert!(!f.fresh_v3(f.candidate).await);
    let visible: bool = sqlx::query_scalar("SELECT ai_first_queue_visible_v3($1,$2,$3)")
        .bind(f.project_id)
        .bind(f.report_id)
        .bind(f.candidate.as_uuid())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        !visible,
        "the report cannot return to the same unresolved person"
    );

    map_legacy_actor(
        &f.pool,
        "user",
        "reviewer-a",
        f.candidate,
        f.operator,
        "confirmed audit label ownership",
    )
    .await
    .unwrap();
    assert!(!f.fresh_v3(f.candidate).await);
    let still_hidden: bool = sqlx::query_scalar("SELECT ai_first_queue_visible_v3($1,$2,$3)")
        .bind(f.project_id)
        .bind(f.report_id)
        .bind(f.candidate.as_uuid())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(!still_hidden);
}

#[tokio::test]
async fn n_minus_one_functions_resolve_mapped_callers_against_canonical_rows() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "unused-owner-label").await;
    sqlx::query(
        "UPDATE ai_screening_cohorts SET approved_by=$2,approved_by_actor_user_id=$3 WHERE id=$1",
    )
    .bind(f.cohort_id)
    .bind(f.candidate.as_uuid().to_string())
    .bind(f.candidate.as_uuid())
    .execute(&f.pool)
    .await
    .unwrap();
    map_legacy_actor(
        &f.pool,
        "user",
        "reviewer-a",
        f.candidate,
        f.operator,
        "N-1 label maps to the N identity",
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO ai_screening_audit_labels(project_id,cohort_id,report_id,actor_id,actor_user_id,decision)
        VALUES($1,$2,$3,$4,$5,'include')")
        .bind(f.project_id)
        .bind(f.cohort_id)
        .bind(f.report_id)
        .bind(f.candidate.as_uuid().to_string())
        .bind(f.candidate.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();

    let same_person_fresh: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind("reviewer-a")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        !same_person_fresh,
        "mapped N-1 identity matches N provenance"
    );
    let same_person_visible: bool = sqlx::query_scalar("SELECT ai_first_queue_visible($1,$2,$3)")
        .bind(f.project_id)
        .bind(f.report_id)
        .bind("reviewer-a")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        !same_person_visible,
        "the N-written label stays hidden from its reviewer"
    );

    map_legacy_actor(
        &f.pool,
        "user",
        "reviewer-b",
        f.owner,
        f.operator,
        "separate N-1 reviewer",
    )
    .await
    .unwrap();
    let other_person_fresh: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind("reviewer-b")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let other_person_visible: bool = sqlx::query_scalar("SELECT ai_first_queue_visible($1,$2,$3)")
        .bind(f.project_id)
        .bind(f.report_id)
        .bind("reviewer-b")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(other_person_fresh);
    assert!(other_person_visible);
}

#[tokio::test]
async fn n_minus_one_freshness_checks_canonical_audit_labels() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-approver").await;
    f.set_approver(f.owner).await;
    map_legacy_actor(
        &f.pool,
        "user",
        "reviewer-a",
        f.candidate,
        f.operator,
        "N-1 label maps to the reviewer identity",
    )
    .await
    .unwrap();

    let fresh_before_label: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind("reviewer-a")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(fresh_before_label);

    sqlx::query("INSERT INTO ai_screening_audit_labels(project_id,cohort_id,report_id,actor_id,actor_user_id,decision)
        VALUES($1,$2,$3,$4,$5,'include')")
        .bind(f.project_id)
        .bind(f.cohort_id)
        .bind(f.report_id)
        .bind(f.candidate.as_uuid().to_string())
        .bind(f.candidate.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();

    let fresh_after_label: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind("reviewer-a")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let visible_after_label: bool = sqlx::query_scalar("SELECT ai_first_queue_visible($1,$2,$3)")
        .bind(f.project_id)
        .bind(f.report_id)
        .bind("reviewer-a")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        !fresh_after_label,
        "old direct writes see canonical audit identity"
    );
    assert!(
        !visible_after_label,
        "old queue hides the reviewer's own label"
    );
}

#[tokio::test]
async fn n_minus_one_unmapped_caller_fails_closed_after_canonical_write() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-approver").await;
    f.set_approver(f.owner).await;
    record_legacy_actor_non_identity(
        &f.pool,
        "user",
        "unmapped-rollback-caller",
        f.owner,
        f.operator,
        "rollback caller is distinct from the canonical approver",
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO screening_events(id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id,actor_user_id)
        VALUES($1,$2,$3,'title_abstract','include',$4,'user',$5,$6)")
        .bind(Uuid::new_v4())
        .bind(f.project_id)
        .bind(f.report_id)
        .bind(f.protocol_id)
        .bind(f.candidate.as_uuid().to_string())
        .bind(f.candidate.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();
    let legacy_fresh: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind("unmapped-rollback-caller")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let legacy_visible: bool = sqlx::query_scalar("SELECT ai_first_queue_visible($1,$2,$3)")
        .bind(f.project_id)
        .bind(f.report_id)
        .bind("unmapped-rollback-caller")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(!legacy_fresh, "N-1 cannot attribute an N-written identity");
    assert!(
        !legacy_visible,
        "rollback workers cannot reopen the canonical row"
    );
}

#[tokio::test]
async fn n_minus_one_resolves_legacy_exposure_labels_conservatively() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-approver").await;
    f.insert_audience_exposure("legacy-exposure-audience").await;
    let exposure_match: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(f.cohort_id)
        .bind("legacy-exposure-audience")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(
        !exposure_match,
        "the old signature still excludes an exposed label"
    );
}

#[tokio::test]
async fn the_n_minus_one_queue_still_matches_legacy_audit_labels() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "cohort-approver").await;
    map_legacy_actor(
        &f.pool,
        "user",
        "cohort-approver",
        f.owner,
        f.operator,
        "known cohort owner",
    )
    .await
    .unwrap();
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
    assert!(
        !another_label,
        "an unmapped label conservatively blocks an unmapped N-1 caller"
    );
}

#[tokio::test]
async fn identity_resolution_uses_provenance_or_exact_operator_mapping_only() {
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
    let uuid_text_mapping: bool =
        sqlx::query_scalar("SELECT legacy_human_actor_matches_user('user',$1,$2)")
            .bind(f.candidate.as_uuid().to_string())
            .bind(f.candidate.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let canonical_provenance: bool =
        sqlx::query_scalar("SELECT actor_identity_may_be_user('user',$1,$2,$2)")
            .bind(f.candidate.as_uuid().to_string())
            .bind(f.candidate.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(mapped);
    assert!(!uuid_text_mapping, "UUID text alone is not a mapping");
    assert!(
        canonical_provenance,
        "trusted actor_user_id identifies the user"
    );
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
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,NULL,'user',$2,$3)")
            .bind(label)
            .bind(f.candidate.as_uuid().to_string())
            .bind(f.candidate.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let other_match: bool =
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,NULL,'user',$2,$3)")
            .bind(label)
            .bind(f.owner.as_uuid().to_string())
            .bind(f.owner.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(mapped_match);
    assert!(!other_match, "a mapping to someone else is not a match");

    let unknown = "unmapped-frozen-label";
    let conservative_match: bool =
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,NULL,'user',$2,$3)")
            .bind(unknown)
            .bind(f.candidate.as_uuid().to_string())
            .bind(f.candidate.as_uuid())
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
        sqlx::query_scalar("SELECT same_human_actor_identity('user',$1,NULL,'user',$2,$3)")
            .bind(unknown)
            .bind(f.candidate.as_uuid().to_string())
            .bind(f.candidate.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(
        denied_match,
        "a non-identity decision cannot resolve an unknown label to a different user"
    );
}

#[tokio::test]
async fn automation_audience_does_not_contaminate_human_frozen_evidence() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "known-owner").await;
    f.set_approver(f.owner).await;
    sqlx::query("INSERT INTO ai_opinion_exposures(project_id,report_id,stage,exposure_source,audience_actor_kind,audience_actor_id,exposure_possible_at)
        VALUES($1,$2,'title_abstract','workflow_run_output','automation','automation-7',now())")
        .bind(f.project_id)
        .bind(f.report_id)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ai_screening_audit_labels(project_id,cohort_id,report_id,actor_id,actor_user_id,decision)
        VALUES($1,$2,$3,$4,$5,'include')")
        .bind(f.project_id)
        .bind(f.cohort_id)
        .bind(f.report_id)
        .bind(f.candidate.as_uuid().to_string())
        .bind(f.candidate.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();

    let contaminated: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ai_screening_audit_labels l
         JOIN ai_opinion_exposures e ON e.project_id=l.project_id AND e.report_id=l.report_id
         WHERE l.cohort_id=$1
           AND (e.audience_actor_id IS NULL OR same_human_actor_identity(
             e.audience_actor_kind,e.audience_actor_id,e.audience_actor_user_id,
             'user',l.actor_id,l.actor_user_id)))",
    )
    .bind(f.cohort_id)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(!contaminated, "automation exposure is not human exposure");
}

#[tokio::test]
async fn legacy_history_stays_unmodified_and_new_writes_can_add_provenance() {
    let Some(pool) = database().await else { return };
    let f = Fixture::new(pool, "historical-approved-by").await;
    let screening_id = f.insert_event("legacy-screening-label").await;
    let exposure_id = f.insert_audience_exposure("legacy-exposure-label").await;
    let label_id = f.insert_label("legacy-audit-label").await;
    let before = historical_rows(&f, screening_id, exposure_id, label_id).await;

    map_legacy_actor(
        &f.pool,
        "user",
        "legacy-audit-label",
        f.owner,
        f.operator,
        "resolve historical audit reviewer before a distinct write",
    )
    .await
    .unwrap();

    let legacy_provenance: (Option<Uuid>, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT
           (SELECT actor_user_id FROM screening_events WHERE id=$1),
           (SELECT audience_actor_user_id FROM ai_opinion_exposures WHERE id=$2),
           (SELECT actor_user_id FROM ai_screening_audit_labels WHERE id=$3)",
    )
    .bind(screening_id)
    .bind(exposure_id)
    .bind(label_id)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(legacy_provenance, (None, None, None));

    let canonical_actor_id = f.candidate.as_uuid().to_string();
    let new_screening_id = Uuid::new_v4();
    sqlx::query("INSERT INTO screening_events(id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id,actor_user_id)
        VALUES($1,$2,$3,'title_abstract','include',$4,'user',$5,$6)")
        .bind(new_screening_id)
        .bind(f.project_id)
        .bind(f.report_id)
        .bind(f.protocol_id)
        .bind(&canonical_actor_id)
        .bind(f.candidate.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();
    let new_exposure_id = Uuid::new_v4();
    sqlx::query("INSERT INTO ai_opinion_exposures(id,project_id,report_id,stage,exposure_source,audience_actor_kind,audience_actor_id,audience_actor_user_id,exposure_possible_at)
        VALUES($1,$2,$3,'title_abstract','workflow_run_output','user',$4,$5,now())")
        .bind(new_exposure_id)
        .bind(f.project_id)
        .bind(f.report_id)
        .bind(&canonical_actor_id)
        .bind(f.candidate.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();
    let new_label_id = Uuid::new_v4();
    sqlx::query("INSERT INTO ai_screening_audit_labels(id,project_id,cohort_id,report_id,actor_id,actor_user_id,decision)
        VALUES($1,$2,$3,$4,$5,$6,'include')")
        .bind(new_label_id)
        .bind(f.project_id)
        .bind(f.cohort_id)
        .bind(f.report_id)
        .bind(&canonical_actor_id)
        .bind(f.candidate.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_ne!(new_screening_id, screening_id);
    assert_ne!(new_exposure_id, exposure_id);
    assert_ne!(new_label_id, label_id);
    let after_new_write = historical_rows(&f, screening_id, exposure_id, label_id).await;
    assert_eq!(after_new_write, before, "new writes do not rewrite history");
    let new_provenance: (Option<Uuid>, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT
           (SELECT actor_user_id FROM screening_events WHERE id=$1),
           (SELECT audience_actor_user_id FROM ai_opinion_exposures WHERE id=$2),
           (SELECT actor_user_id FROM ai_screening_audit_labels WHERE id=$3)",
    )
    .bind(new_screening_id)
    .bind(new_exposure_id)
    .bind(new_label_id)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        new_provenance,
        (
            Some(f.candidate.as_uuid()),
            Some(f.candidate.as_uuid()),
            Some(f.candidate.as_uuid())
        )
    );
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
    sqlx::query(
        "UPDATE ai_screening_cohorts SET approved_by=$2,approved_by_actor_user_id=$3 WHERE id=$1",
    )
    .bind(f.cohort_id)
    .bind(f.owner.as_uuid().to_string())
    .bind(f.owner.as_uuid())
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
    let counted = f.fresh_v3(deactivated_user).await;
    assert!(!counted, "the deactivated person's provenance still counts");
    let deactivated: bool =
        sqlx::query_scalar("SELECT status='deactivated' FROM users WHERE id=$1")
            .bind(deactivated_user.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(deactivated);
}
