#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use deepref_app::{
    assign_scientific_reviewer,
    identity::{development_principal_id, ensure_development_principal},
};
use deepref_domain::{ProjectId, ScientificAssignmentRole, UserId};
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 3).await
}

#[tokio::test]
async fn development_principal_is_gated_and_can_assign_reviewers() {
    let Some(pool) = database().await else { return };

    assert_eq!(
        ensure_development_principal(&pool, false).await.unwrap(),
        None
    );
    let development_user = development_principal_id();
    let before: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
        .bind(development_user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        !before,
        "a disabled development principal is not provisioned"
    );

    assert_eq!(
        ensure_development_principal(&pool, true).await.unwrap(),
        Some(development_user)
    );
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'Development principal assignment')")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    let reviewer = UserId::new(Uuid::new_v4());
    sqlx::query(
        "INSERT INTO users(id,workos_user_id,primary_email,status) VALUES($1,$2,$3,'active')",
    )
    .bind(reviewer.as_uuid())
    .bind(format!("development-reviewer-{}", Uuid::new_v4()))
    .bind(format!(
        "development-reviewer-{}@example.test",
        Uuid::new_v4()
    ))
    .execute(&pool)
    .await
    .unwrap();

    let assignment = assign_scientific_reviewer(
        &pool,
        ProjectId::new(project),
        reviewer,
        ScientificAssignmentRole::FreshAiAuditor,
        development_user,
    )
    .await
    .unwrap();
    assert_eq!(assignment.assigned_by, development_user);
    assert_eq!(assignment.user_id, reviewer);

    let active: bool = sqlx::query_scalar("SELECT status='active' FROM users WHERE id=$1")
        .bind(development_user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(active);
}
