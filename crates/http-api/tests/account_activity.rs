use axum::http::{Method, StatusCode};
use uuid::Uuid;

mod support;
use support::authz::Harness;

#[tokio::test]
async fn authenticated_product_command_records_durable_activity_with_one_connection()
-> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let h = Harness::on(pool);
    let person = h.sign_in("product-activity", "activity@example.org").await;
    let project = h.create_project(&person, "Human research").await;
    let previous: Uuid = sqlx::query_scalar(
        "UPDATE account_human_activity SET occurred_at=clock_timestamp()-interval '23 months'
         WHERE user_id=$1 RETURNING generation",
    )
    .bind(person.user.as_uuid())
    .fetch_one(&h.pool)
    .await?;

    let (status, body) = h
        .json(
            &person,
            Method::POST,
            &format!("/projects/{project}/studies"),
            r#"{"title":"A human-created study"}"#,
        )
        .await;
    anyhow::ensure!(
        status == StatusCode::CREATED,
        "genuine product command must succeed before testing evidence: {status}, {body}"
    );
    let (generation, kind, occurred_at): (Uuid, String, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as(
            "SELECT generation,evidence_kind,occurred_at FROM account_human_activity WHERE user_id=$1",
        )
        .bind(person.user.as_uuid())
        .fetch_one(&h.pool)
        .await?;
    anyhow::ensure!(
        generation != previous,
        "durable authenticated product command produced no new human activity evidence"
    );
    anyhow::ensure!(
        kind == "product_command",
        "product command evidence must declare its provenance: {kind}"
    );
    let now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&h.pool)
        .await?;
    anyhow::ensure!(
        occurred_at <= now && occurred_at > now - chrono::Duration::minutes(1),
        "command evidence must use actual database time: evidence={occurred_at}, now={now}"
    );
    Ok(())
}

async fn generation(h: &Harness, person: &support::authz::Person) -> anyhow::Result<Uuid> {
    Ok(
        sqlx::query_scalar("SELECT generation FROM account_human_activity WHERE user_id=$1")
            .bind(person.user.as_uuid())
            .fetch_one(&h.pool)
            .await?,
    )
}

#[tokio::test]
async fn project_edit_is_an_explicit_durable_command_attempt() -> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let h = Harness::on(pool);
    let person = h.sign_in("edit-activity", "edit@example.org").await;
    let project = h.create_project(&person, "Before human edit").await;
    let before = generation(&h, &person).await?;
    let (status, body) = h
        .json(
            &person,
            Method::PATCH,
            &format!("/projects/{project}"),
            r#"{"name":"After human edit"}"#,
        )
        .await;
    anyhow::ensure!(
        status == StatusCode::OK,
        "real project edit must succeed: {status}, {body}"
    );
    anyhow::ensure!(
        generation(&h, &person).await? != before,
        "successful project edit has no durable command-attempt evidence"
    );
    Ok(())
}

#[tokio::test]
async fn evidence_storage_failure_prevents_the_business_mutation() -> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let h = Harness::on(pool);
    let person = h.sign_in("storage-activity", "storage@example.org").await;
    let project = h.create_project(&person, "Receipt storage failure").await;
    let before = generation(&h, &person).await?;
    // Owned isolated schema only: make the evidence write fail without
    // changing the actual session, PDP, handler or business-table permissions.
    sqlx::query("ALTER TABLE account_human_activity ADD CONSTRAINT fixture_refuses_command_evidence CHECK(evidence_kind <> 'product_command') NOT VALID")
        .execute(&h.pool).await?;
    let (status, _) = h
        .json(
            &person,
            Method::POST,
            &format!("/projects/{project}/studies"),
            r#"{"title":"Must not commit without a receipt"}"#,
        )
        .await;
    anyhow::ensure!(
        status == StatusCode::INTERNAL_SERVER_ERROR,
        "storage refusal must be an honest server error: {status}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM studies WHERE project_id=$1")
        .bind(project)
        .fetch_one(&h.pool)
        .await?;
    anyhow::ensure!(
        count == 0,
        "handler committed {count} studies without durable evidence"
    );
    anyhow::ensure!(
        generation(&h, &person).await? == before,
        "failed receipt changed generation"
    );
    Ok(())
}

#[tokio::test]
async fn rejected_body_is_an_attempt_not_a_business_success() -> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let h = Harness::on(pool);
    let person = h.sign_in("invalid-activity", "invalid@example.org").await;
    let project = h.create_project(&person, "Invalid human command").await;
    let before = generation(&h, &person).await?;
    let (status, _) = h
        .json(
            &person,
            Method::POST,
            &format!("/projects/{project}/studies"),
            "{}",
        )
        .await;
    anyhow::ensure!(
        status.is_client_error(),
        "body must actually be rejected: {status}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM studies WHERE project_id=$1")
        .bind(project)
        .fetch_one(&h.pool)
        .await?;
    anyhow::ensure!(count == 0, "invalid command created {count} studies");
    anyhow::ensure!(
        generation(&h, &person).await? != before,
        "actual authenticated attempt must be distinguished from business success"
    );
    Ok(())
}

#[tokio::test]
async fn reads_denied_tenants_and_background_provenance_are_not_activity() -> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let h = Harness::on(pool);
    let person = h.sign_in("passive-activity", "passive@example.org").await;
    let other = h.sign_in("foreign-activity", "foreign@example.org").await;
    let project = h.create_project(&person, "Passive traffic").await;
    let before = generation(&h, &person).await?;
    for path in [
        "/me".to_owned(),
        format!("/projects/{project}"),
        "/notifications/unread-count".to_owned(),
    ] {
        let (status, _) = h.get(&person, &path).await;
        anyhow::ensure!(
            status == StatusCode::OK,
            "actual read {path} failed: {status}"
        );
    }
    let (status, _) = h
        .json(
            &person,
            Method::POST,
            "/notifications/mark-read",
            r#"{"all":true}"#,
        )
        .await;
    anyhow::ensure!(
        status == StatusCode::OK,
        "notification acknowledgement failed: {status}"
    );
    let other_before = generation(&h, &other).await?;
    let (status, _) = h
        .json(
            &other,
            Method::POST,
            &format!("/projects/{project}/studies"),
            r#"{"title":"Foreign attempt"}"#,
        )
        .await;
    anyhow::ensure!(
        status == StatusCode::NOT_FOUND,
        "foreign central PDP denial changed: {status}"
    );
    anyhow::ensure!(
        generation(&h, &other).await? == other_before,
        "central-PDP denied dispatch produced evidence"
    );
    // A background/application command retains its true human scientific
    // actor. That provenance must never manufacture a fresh human interaction.
    deepref_app::create_study(
        &h.pool,
        deepref_application::CreateStudy {
            project_id: deepref_domain::ProjectId::new(project),
            study_id: deepref_domain::StudyId::new(Uuid::new_v4()),
            title: deepref_domain::StudyTitle::new(
                "Background result attributed to its real actor",
            )?,
            actor: deepref_domain::Actor::authenticated_user(person.user),
        },
    )
    .await?;
    anyhow::ensure!(
        generation(&h, &person).await? == before,
        "read/poll/acknowledgement/background work manufactured human activity"
    );
    Ok(())
}

#[tokio::test]
async fn current_session_fences_reject_wrong_revoked_expired_and_inactive_users()
-> anyhow::Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let h = Harness::on(pool);
    let person = h.sign_in("session-activity", "session@example.org").await;
    let other = h
        .sign_in("other-session-activity", "other-session@example.org")
        .await;
    let own: Uuid = sqlx::query_scalar("SELECT id FROM user_sessions WHERE user_id=$1")
        .bind(person.user.as_uuid())
        .fetch_one(&h.pool)
        .await?;
    let foreign: Uuid = sqlx::query_scalar("SELECT id FROM user_sessions WHERE user_id=$1")
        .bind(other.user.as_uuid())
        .fetch_one(&h.pool)
        .await?;
    let before = generation(&h, &person).await?;
    anyhow::ensure!(
        !deepref_app::account_activity::record_product_command_attempt(
            &h.pool,
            person.user,
            foreign
        )
        .await?,
        "another user's live session was accepted"
    );
    sqlx::query("UPDATE user_sessions SET revoked_at=clock_timestamp() WHERE id=$1")
        .bind(own)
        .execute(&h.pool)
        .await?;
    anyhow::ensure!(
        !deepref_app::account_activity::record_product_command_attempt(&h.pool, person.user, own)
            .await?,
        "revoked session was accepted"
    );
    // Preserve expires_at>created_at while setting a genuinely expired row.
    sqlx::query("UPDATE user_sessions SET revoked_at=NULL,created_at=clock_timestamp()-interval '2 days',expires_at=clock_timestamp()-interval '1 day' WHERE id=$1")
        .bind(own).execute(&h.pool).await?;
    anyhow::ensure!(
        !deepref_app::account_activity::record_product_command_attempt(&h.pool, person.user, own)
            .await?,
        "expired session was accepted"
    );
    sqlx::query(
        "UPDATE user_sessions SET expires_at=clock_timestamp()+interval '1 day' WHERE id=$1",
    )
    .bind(own)
    .execute(&h.pool)
    .await?;
    sqlx::query("UPDATE users SET status='deactivated' WHERE id=$1")
        .bind(person.user.as_uuid())
        .execute(&h.pool)
        .await?;
    anyhow::ensure!(
        !deepref_app::account_activity::record_product_command_attempt(&h.pool, person.user, own)
            .await?,
        "inactive user was accepted through a stale session"
    );
    anyhow::ensure!(
        generation(&h, &person).await? == before,
        "a denied fence changed activity"
    );
    Ok(())
}

#[tokio::test]
async fn machine_key_reads_and_spoofed_human_headers_do_not_refresh_activity() -> anyhow::Result<()>
{
    use axum::{
        body::Body,
        http::{Request, header},
    };
    use tower::ServiceExt;
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let h = Harness::on(pool);
    let person = h.sign_in("machine-activity", "machine@example.org").await;
    let project = h.create_project(&person, "Machine traffic").await;
    let (status, body) = h
        .json(
            &person,
            Method::POST,
            &format!("/workspaces/{}/api-keys", person.workspace.as_uuid()),
            r#"{"name":"read automation","scopes":["project:view"]}"#,
        )
        .await;
    anyhow::ensure!(
        status == StatusCode::CREATED,
        "human key creation failed: {status}"
    );
    let secret = body["secret"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("key secret was not returned"))?;
    let before = generation(&h, &person).await?;
    for (method, path, expected) in [
        (Method::GET, format!("/projects/{project}"), StatusCode::OK),
        (
            Method::POST,
            format!("/projects/{project}/studies"),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let response = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                    .header("x-actor-kind", "user")
                    .header("x-actor-id", person.user.as_uuid().to_string())
                    .body(Body::empty())?,
            )
            .await?;
        anyhow::ensure!(
            response.status() == expected,
            "machine policy outcome changed: {}",
            response.status()
        );
    }
    anyhow::ensure!(
        generation(&h, &person).await? == before,
        "machine traffic with human-looking headers manufactured human activity"
    );
    Ok(())
}

struct RevokeAfterPermission {
    inner: std::sync::Arc<dyn deepref_application::authorization::Authorizer>,
    pool: sqlx::PgPool,
}
impl deepref_application::authorization::Authorizer for RevokeAfterPermission {
    fn require<'a>(
        &'a self,
        principal: &'a deepref_domain::Principal,
        permission: deepref_domain::authorization::Permission,
        resource: deepref_domain::authorization::Resource,
    ) -> deepref_application::authorization::AuthorizationFuture<
        'a,
        Result<(), deepref_application::authorization::AuthzError>,
    > {
        Box::pin(async move {
            // This is a real current PDP allowance, followed by deterministic
            // revocation at the public Authorizer seam; never a fake grant.
            self.inner.require(principal, permission, resource).await?;
            if permission == deepref_domain::authorization::Permission::ProjectEdit
                && let Some(user) = principal.user_id()
            {
                sqlx::query(
                    "UPDATE user_sessions SET revoked_at=clock_timestamp() WHERE user_id=$1",
                )
                .bind(user.as_uuid())
                .execute(&self.pool)
                .await
                .map_err(|_| {
                    deepref_application::authorization::AuthzError::Store(
                        "fixture revocation failed".into(),
                    )
                })?;
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn revocation_after_current_pdp_allowance_prevents_evidence_and_handler() -> anyhow::Result<()>
{
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await
    else {
        return Ok(());
    };
    let h = Harness::with_state(pool, |state| {
        let authorizer = std::sync::Arc::new(RevokeAfterPermission {
            inner: state.authorizer.clone(),
            pool: state.pool.clone(),
        });
        state.with_authorizer(authorizer)
    });
    let person = h.sign_in("revoked-activity", "revoked@example.org").await;
    let project = h.create_project(&person, "Revocation before receipt").await;
    let before = generation(&h, &person).await?;
    let (status, _) = h
        .json(
            &person,
            Method::POST,
            &format!("/projects/{project}/studies"),
            r#"{"title":"Must not commit after session revocation"}"#,
        )
        .await;
    anyhow::ensure!(
        status == StatusCode::UNAUTHORIZED,
        "revocation fence did not refuse dispatch: {status}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM studies WHERE project_id=$1")
        .bind(project)
        .fetch_one(&h.pool)
        .await?;
    anyhow::ensure!(
        count == 0 && generation(&h, &person).await? == before,
        "stale authenticated context produced activity or business effects after revocation"
    );
    Ok(())
}
