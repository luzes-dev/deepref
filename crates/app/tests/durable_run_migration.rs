#![allow(clippy::expect_used, clippy::unwrap_used)]
use deepref_app::MIGRATOR;
use serde_json::json;
use sqlx::{Row, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

async fn historical_pool() -> Option<sqlx::PgPool> {
    let url = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("historical database");
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|m| m.version < 73)
            .cloned()
            .collect(),
    )
    .run(&pool)
    .await
    .expect("historical migrations");
    Some(pool)
}

async fn seed(pool: &sqlx::PgPool, collision: bool) -> (Uuid, Uuid, Uuid, Uuid, Uuid, Uuid) {
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'durable migration')")
        .bind(project)
        .execute(pool)
        .await
        .expect("project");
    let definition:Uuid=sqlx::query_scalar("SELECT id FROM configure_automation_definition($1,'maintenance','manual','project_maintenance',1,'active','user','migration')")
        .bind(project).fetch_one(pool).await.expect("definition");
    let automation =
        sqlx::query("SELECT * FROM start_automation_manually($1,$2,'identity','user','migration')")
            .bind(project)
            .bind(definition)
            .fetch_one(pool)
            .await
            .expect("legacy run");
    let run: Uuid = automation.get("run_id");
    let job: Uuid = automation.get("job_id");
    let step: Uuid = sqlx::query_scalar(
        "SELECT id FROM automation_step_runs WHERE automation_run_id=$1 LIMIT 1",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .expect("legacy step");
    sqlx::query("UPDATE jobs SET state='running',attempts=1,lease_owner='migration-worker',leased_until=now()+interval '5 minutes',lease_renewed_at=now() WHERE id=$1")
        .bind(job).execute(pool).await.expect("active lease");
    sqlx::query("UPDATE automation_runs SET status='running',started_at=now() WHERE id=$1")
        .bind(run)
        .execute(pool)
        .await
        .expect("run starts");
    sqlx::query("UPDATE automation_step_runs SET status='running',attempts=1,claimed_by='migration-worker',claimed_job_attempt=1,started_at=now() WHERE id=$1")
        .bind(step).execute(pool).await.expect("step starts");
    sqlx::query("INSERT INTO review_run_manifests(project_id,automation_run_id,definition_key,definition_id,definition_version,manifest_hash,semantic_bundle_hash,manifest,subject,origin,prepared_task,state,started_at,generation,reuse_epoch)
        VALUES($1,$2,'duplicate_detection','duplicate_detection',1,$3,$3,'{}','{}','{}','{}','running',now(),2,1)")
        .bind(project).bind(run).bind("a".repeat(64)).execute(pool).await.expect("review detail");
    let artifact = Uuid::new_v4();
    sqlx::query("INSERT INTO review_artifacts(id,project_id,content_hash,media_type,payload) VALUES($1,$2,$3,'application/json','{}')")
        .bind(artifact).bind(project).bind("b".repeat(64)).execute(pool).await.expect("artifact");
    let attempt = Uuid::new_v4();
    sqlx::query("INSERT INTO review_step_attempts(id,project_id,automation_run_id,node_id,node_version,attempt_number,input_fingerprint,status,worker_id,artifact_id,finished_at,accepted_at)
        VALUES($1,$2,$3,'prepare',1,1,$4,'completed','migration-worker',$5,now(),now())")
        .bind(attempt).bind(project).bind(run).bind("c".repeat(64)).bind(artifact).execute(pool).await.expect("accepted attempt");
    sqlx::query(
        "UPDATE automation_step_runs SET accepted_attempt_id=$2,input_fingerprint=$3 WHERE id=$1",
    )
    .bind(step)
    .bind(attempt)
    .bind("c".repeat(64))
    .execute(pool)
    .await
    .expect("accepted binding");
    let workflow = Uuid::new_v4();
    sqlx::query("INSERT INTO workflows(id,project_id,name,webhook_token,webhook_secret,email_token,created_by_kind,created_by_id)
        VALUES($1,$2,'saved graph',$3,'secret',$4,'user','migration')")
        .bind(workflow).bind(project).bind(Uuid::new_v4().to_string()).bind(Uuid::new_v4().to_string()).execute(pool).await.expect("workflow");
    let workflow_run = if collision { run } else { Uuid::new_v4() };
    sqlx::query("INSERT INTO workflow_runs(id,project_id,workflow_id,graph,trigger_kind,idempotency_key,actor_kind,actor_id,status,started_at)
        VALUES($1,$2,$3,'{}','manual','identity','user','migration','running',now())")
        .bind(workflow_run).bind(project).bind(workflow).execute(pool).await.expect("workflow run");
    let node = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow_node_runs(id,project_id,run_id,node_id,node_type,status,wait_state)
        VALUES($1,$2,$3,'wait','ai.run_review','queued',$4)",
    )
    .bind(node)
    .bind(project)
    .bind(workflow_run)
    .bind(json!({"poll":2}))
    .execute(pool)
    .await
    .expect("waiting node");
    sqlx::query(
        "INSERT INTO jobs(id,project_id,kind,payload,dedupe_key,max_attempts)
        VALUES($1,$2,'workflow_step',$3,$4,4)",
    )
    .bind(Uuid::new_v4())
    .bind(project)
    .bind(json!({"node_run_id":node}))
    .bind(format!("workflow_step:{node}:wait:2"))
    .execute(pool)
    .await
    .expect("queued look");
    sqlx::query("INSERT INTO workflow_files(project_id,run_id,node_id,name,content_type,content) VALUES($1,$2,'export','result','text/plain',$3)")
        .bind(project).bind(workflow_run).bind(b"preserved".as_slice()).execute(pool).await.expect("run FK");
    (run, step, job, workflow_run, node, attempt)
}

#[tokio::test]
async fn populated_cutover_preserves_identity_live_leases_waits_and_accepted_evidence() {
    let Some(pool) = historical_pool().await else {
        return;
    };
    let (run, step, job, workflow_run, node, attempt) = seed(&pool, false).await;
    let lease_before =
        sqlx::query("SELECT lease_owner,leased_until,attempts,dedupe_key FROM jobs WHERE id=$1")
            .bind(job)
            .fetch_one(&pool)
            .await
            .expect("lease before");
    MIGRATOR.run(&pool).await.expect("full durable cutover");
    let persisted=sqlx::query("SELECT j.*,s.run_id FROM jobs j JOIN durable_steps s ON s.id=(j.payload->>'step_id')::uuid WHERE j.id=$1")
        .bind(job).fetch_one(&pool).await.expect("same job");
    assert_eq!(persisted.get::<String, _>("kind"), "durable_run_step");
    assert_eq!(persisted.get::<Uuid, _>("run_id"), run);
    assert_eq!(
        persisted.get::<String, _>("lease_owner"),
        lease_before.get::<String, _>("lease_owner")
    );
    assert_eq!(
        persisted.get::<chrono::DateTime<chrono::Utc>, _>("leased_until"),
        lease_before.get::<chrono::DateTime<chrono::Utc>, _>("leased_until")
    );
    assert_eq!(
        persisted.get::<i32, _>("attempts"),
        lease_before.get::<i32, _>("attempts")
    );
    assert_eq!(
        persisted.get::<String, _>("dedupe_key"),
        lease_before.get::<String, _>("dedupe_key")
    );
    let binding: Uuid =
        sqlx::query_scalar("SELECT accepted_attempt_id FROM automation_step_details WHERE id=$1")
            .bind(step)
            .fetch_one(&pool)
            .await
            .expect("same step binding");
    assert_eq!(binding, attempt);
    let epoch: (i32, i32) = sqlx::query_as(
        "SELECT generation,reuse_epoch FROM review_run_manifests WHERE automation_run_id=$1",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .expect("review identity");
    assert_eq!(epoch, (2, 1));
    let waiting: serde_json::Value = sqlx::query_scalar(
        "SELECT wait_state FROM workflow_step_details WHERE id=$1 AND run_id=$2",
    )
    .bind(node)
    .bind(workflow_run)
    .fetch_one(&pool)
    .await
    .expect("same waiting step");
    assert_eq!(waiting, json!({"poll":2}));
    let file: Uuid = sqlx::query_scalar("SELECT run_id FROM workflow_files WHERE run_id=$1")
        .bind(workflow_run)
        .fetch_one(&pool)
        .await
        .expect("FK preserved");
    assert_eq!(file, workflow_run);
    let obsolete:i64=sqlx::query_scalar("SELECT count(*) FROM pg_class WHERE relnamespace='public'::regnamespace AND relname=ANY($1)")
        .bind(vec!["automation_runs","automation_step_runs","workflow_runs","workflow_node_runs","review_step_attempts"])
        .fetch_one(&pool).await.expect("retired relations");
    assert_eq!(obsolete, 0);
    let invalid:i64=sqlx::query_scalar("SELECT count(*) FROM pg_constraint WHERE contype='f' AND NOT convalidated AND confrelid IN ('durable_runs'::regclass,'durable_steps'::regclass,'durable_attempts'::regclass)")
        .fetch_one(&pool).await.expect("validated FKs");
    assert_eq!(invalid, 0);
}

#[tokio::test]
async fn cross_engine_uuid_collision_aborts_without_remapping_or_dropping_sources() {
    let Some(pool) = historical_pool().await else {
        return;
    };
    let (run, _, _, _, _, _) = seed(&pool, true).await;
    assert!(MIGRATOR.run(&pool).await.is_err());
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM automation_runs a JOIN workflow_runs w USING(id) WHERE a.id=$1",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .expect("rollback retains sources");
    assert_eq!(retained, 1);
}

#[tokio::test]
async fn terminal_review_history_closes_the_old_running_envelope_without_replacing_its_lease() {
    let Some(pool) = historical_pool().await else {
        return;
    };
    let (run, step, job, _, _, accepted) = seed(&pool, false).await;
    sqlx::query(
        "UPDATE review_run_manifests SET state='failed',state_code='review_execution_failed',
        state_message='interrupted envelope',finished_at=now() WHERE automation_run_id=$1",
    )
    .bind(run)
    .execute(&pool)
    .await
    .expect("scientific history closes before envelope");
    deepref_app::MIGRATOR
        .run(&pool)
        .await
        .expect("terminal source migrates");
    let state: String = sqlx::query_scalar("SELECT status FROM durable_runs WHERE id=$1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .expect("canonical state");
    assert_eq!(state, "failed");
    let unfinished: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM durable_steps WHERE run_id=$1
        AND status IN ('pending','queued','running','waiting')",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .expect("step closure");
    assert_eq!(unfinished, 0);
    let unfinished: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM durable_attempts WHERE run_id=$1
        AND status IN ('running','waiting')",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .expect("attempt closure");
    assert_eq!(unfinished, 0);
    let accepted_state: String =
        sqlx::query_scalar("SELECT status FROM durable_attempts WHERE id=$1")
            .bind(accepted)
            .fetch_one(&pool)
            .await
            .expect("accepted identity retained");
    assert_eq!(accepted_state, "completed");
    let lease = sqlx::query("SELECT kind,state,lease_owner,attempts,payload FROM jobs WHERE id=$1")
        .bind(job)
        .fetch_one(&pool)
        .await
        .expect("original queue row");
    assert_eq!(lease.get::<String, _>("kind"), "durable_run_step");
    assert_eq!(lease.get::<String, _>("state"), "running");
    assert_eq!(lease.get::<String, _>("lease_owner"), "migration-worker");
    assert_eq!(lease.get::<i32, _>("attempts"), 1);
    assert_eq!(
        lease.get::<serde_json::Value, _>("payload"),
        json!({
            "run_id":run,"step_id":step,"handler_kind":"automation"
        })
    );
    let mut tx = pool.begin().await.expect("old worker transaction");
    let lease = deepref_application::jobs::JobLease {
        id: job,
        owner: "migration-worker".into(),
        attempt: 1,
    };
    assert!(
        deepref_app::durable_runs::begin_step(&mut tx, step, &lease)
            .await
            .expect("terminal fence")
            .is_none(),
        "a still-live old job cannot reopen the terminal run"
    );
    tx.commit()
        .await
        .expect("read-only claim rejection commits");
}
