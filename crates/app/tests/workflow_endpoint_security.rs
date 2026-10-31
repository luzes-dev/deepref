#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Workflow tenancy and hook credentials: a graph may only name extraction
//! fields of its own project (a foreign or missing one is refused alike), and
//! rotating a hook credential invalidates the old one at once and leaves an
//! audit row that never contains the credential.

use deepref_app::{
    RotateTarget, UpdateWorkflow, WorkflowError, create_workflow, find_workflow_by_email_token,
    find_workflow_by_webhook_token, get_webhook_config, identity::AuditRequest, publish_workflow,
    rotate_endpoint_credentials, update_workflow,
};
use deepref_application::workflows::{Endpoint, GraphEdge, GraphNode, Position, WorkflowGraph};
use deepref_domain::{Actor, ProjectId, UserId};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 5).await
}

struct Tenant {
    workspace: Uuid,
    project: ProjectId,
    user: UserId,
}

async fn tenant(pool: &PgPool) -> Tenant {
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,workos_user_id,primary_email,status) VALUES ($1,$2,'wf@example.test','active')")
        .bind(user).bind(format!("wf-{user}")).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO workspaces (id,kind,name,slug) VALUES ($1,'organization','wf',$2)")
        .bind(workspace)
        .bind(format!("wf-{workspace}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO projects (id,name,workspace_id) VALUES ($1,$2,$3)")
        .bind(project)
        .bind(format!("wf-{project}"))
        .bind(workspace)
        .execute(pool)
        .await
        .unwrap();
    Tenant {
        workspace,
        project: ProjectId::new(project),
        user: UserId::new(user),
    }
}

async fn extraction_field(pool: &PgPool, project: ProjectId) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO extraction_field_definitions (id,project_id,version,field_key,label,value_type) VALUES ($1,$2,1,'n','N','number')")
        .bind(id).bind(project.as_uuid()).execute(pool).await.unwrap();
    id
}

fn node(id: &str, node_type: &str, config: Value) -> GraphNode {
    GraphNode {
        id: id.to_owned(),
        node_type: node_type.to_owned(),
        position: Position::default(),
        label: None,
        config,
    }
}

fn extract_graph(field: &str) -> WorkflowGraph {
    WorkflowGraph {
        nodes: vec![
            node("t", "trigger.manual", json!({})),
            node("x", "ai.extract_field", json!({ "field": field })),
        ],
        edges: vec![GraphEdge {
            id: None,
            from: Endpoint {
                node: "t".to_owned(),
                port: "records".to_owned(),
            },
            to: Endpoint {
                node: "x".to_owned(),
                port: "records".to_owned(),
            },
        }],
    }
}

fn actor(tenant: &Tenant) -> Actor {
    Actor::authenticated_user(tenant.user)
}

fn update(graph: WorkflowGraph) -> UpdateWorkflow {
    UpdateWorkflow {
        niche_nodes_enabled: true,
        name: None,
        description: None,
        graph: Some(graph),
        expected_revision: None,
    }
}

#[tokio::test]
async fn graphs_refuse_extraction_fields_of_other_projects_like_missing_ones() {
    let Some(pool) = database().await else { return };
    let ours = tenant(&pool).await;
    let theirs = tenant(&pool).await;
    let own_field = extraction_field(&pool, ours.project).await;
    let foreign_field = extraction_field(&pool, theirs.project).await;
    let missing_field = Uuid::new_v4();

    for refused in [
        foreign_field.to_string(),
        missing_field.to_string(),
        "not-a-uuid".to_owned(),
    ] {
        let created = create_workflow(
            &pool,
            ours.project,
            "Extract",
            "",
            extract_graph(&refused),
            &actor(&ours),
            true,
        )
        .await;
        assert!(
            matches!(created, Err(WorkflowError::ReferenceNotFound)),
            "create with {refused}: {created:?}"
        );
    }

    let workflow = create_workflow(
        &pool,
        ours.project,
        "Extract",
        "",
        extract_graph(&own_field.to_string()),
        &actor(&ours),
        true,
    )
    .await
    .expect("own field is accepted");

    for refused in [foreign_field, missing_field] {
        let saved = update_workflow(
            &pool,
            ours.project,
            workflow.id,
            update(extract_graph(&refused.to_string())),
        )
        .await;
        assert!(
            matches!(saved, Err(WorkflowError::ReferenceNotFound)),
            "update with {refused}: {saved:?}"
        );
    }
    let stored = deepref_app::get_workflow(&pool, ours.project, workflow.id)
        .await
        .unwrap();
    assert_eq!(
        stored.draft_graph.nodes[1].config["field"],
        json!(own_field.to_string()),
        "a refused save leaves the draft untouched"
    );

    // A draft that already names a foreign field (written before this check)
    // cannot be published.
    sqlx::query("UPDATE workflows SET draft_graph=$2 WHERE id=$1")
        .bind(workflow.id)
        .bind(serde_json::to_value(extract_graph(&foreign_field.to_string())).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let published = publish_workflow(
        &pool,
        ours.project,
        workflow.id,
        None,
        &actor(&ours),
        false,
        true,
    )
    .await;
    assert!(
        matches!(published, Err(WorkflowError::ReferenceNotFound)),
        "{published:?}"
    );
}

type AuditRow = (String, Option<Uuid>, Option<Uuid>, Option<Uuid>, Value);

fn audit() -> AuditRequest<'static> {
    AuditRequest {
        request_id: Some("rotate-test"),
        session_ref: None,
    }
}

#[tokio::test]
async fn rotating_hook_credentials_invalidates_the_old_ones_and_is_audited_without_them() {
    let Some(pool) = database().await else { return };
    let ours = tenant(&pool).await;
    let graph = WorkflowGraph {
        nodes: vec![node("t", "trigger.webhook", json!({}))],
        edges: vec![],
    };
    let workflow = create_workflow(&pool, ours.project, "Hook", "", graph, &actor(&ours), true)
        .await
        .unwrap();
    let before = get_webhook_config(&pool, ours.project, workflow.id)
        .await
        .unwrap();
    let old_token = before.token.clone();
    let old_email = before.email_token.clone();

    let after = rotate_endpoint_credentials(
        &pool,
        ours.project,
        workflow.id,
        RotateTarget::Webhook,
        ours.user,
        audit(),
    )
    .await
    .unwrap();
    let new_token = after.token.clone();
    assert_ne!(new_token, old_token);
    assert_ne!(after.secret, before.secret);
    assert!(
        find_workflow_by_webhook_token(&pool, &old_token)
            .await
            .unwrap()
            .is_none(),
        "the old webhook address stops working at once"
    );
    assert!(
        find_workflow_by_webhook_token(&pool, &new_token)
            .await
            .unwrap()
            .is_some()
    );

    let after_email = rotate_endpoint_credentials(
        &pool,
        ours.project,
        workflow.id,
        RotateTarget::Email,
        ours.user,
        audit(),
    )
    .await
    .unwrap();
    assert!(
        find_workflow_by_email_token(&pool, &old_email)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(after_email.token, new_token);

    let rows: Vec<AuditRow> = sqlx::query_as(
        "SELECT event_type, principal_user_id, workspace_id, project_id, metadata
         FROM admin_audit_events WHERE project_id=$1 ORDER BY occurred_at, metadata->>'credential'",
    )
    .bind(ours.project.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    let credentials: Vec<&str> = rows
        .iter()
        .map(|row| row.4["credential"].as_str().unwrap())
        .collect();
    assert!(credentials.contains(&"webhook") && credentials.contains(&"email"));
    for (event, principal, workspace, _, metadata) in &rows {
        assert_eq!(event, "workflow.endpoint_rotated");
        assert_eq!(*principal, Some(ours.user.as_uuid()));
        assert_eq!(*workspace, Some(ours.workspace));
        assert_eq!(metadata["workflow_id"], json!(workflow.id.to_string()));
        let text = metadata.to_string();
        for credential in [&old_token, &new_token, &old_email] {
            assert!(
                !text.contains(credential.as_str()),
                "audit must not hold a credential"
            );
        }
    }

    // Another project's caller cannot rotate this workflow, and nothing is audited.
    let theirs = tenant(&pool).await;
    let foreign = rotate_endpoint_credentials(
        &pool,
        theirs.project,
        workflow.id,
        RotateTarget::Webhook,
        theirs.user,
        audit(),
    )
    .await;
    assert!(matches!(foreign, Err(WorkflowError::NotFound)));
    assert!(
        find_workflow_by_webhook_token(&pool, &new_token)
            .await
            .unwrap()
            .is_some()
    );
    let foreign_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM admin_audit_events WHERE project_id=$1")
            .bind(theirs.project.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(foreign_rows, 0);
}
