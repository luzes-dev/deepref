#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! Cross-workspace child access (handoff 03b): acquisitions, AI proposals,
//! AI activity and batches, reviewer decisions and AI-first cohorts.
//!
//! Each test signs in two owners, gives each a project, and writes real child
//! rows into A's project. B then calls a child route under B's project with
//! A's child identifier. Every answer must be byte-identical to the answer for
//! a random UUID, and A's row must be unchanged afterwards. A can still reach
//! the same row, so the comparison is against live data, not a missing fixture.
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

mod support;
use support::authz::{CORRELATION, Harness, Person};
use support::{ORIGIN, SESSION_COOKIE};

struct World {
    h: Harness,
    a: Person,
    b: Person,
    a_project: Uuid,
    b_project: Uuid,
    acquisition: Uuid,
    study: Uuid,
    proposal: Uuid,
    reviewer_decision: Uuid,
    activity: Uuid,
    second_activity: Uuid,
    batch: Uuid,
    cohort: Uuid,
}

async fn world() -> Option<World> {
    let h = Harness::new().await?;
    let a = h.sign_in("user_A", "a@example.org").await;
    let b = h.sign_in("user_B", "b@example.org").await;
    let a_project = h.create_project(&a, "A's project").await;
    let b_project = h.create_project(&b, "B's project").await;

    let report = Uuid::new_v4();
    let protocol = Uuid::new_v4();
    let study = Uuid::new_v4();
    let run = Uuid::new_v4();
    let proposal = Uuid::new_v4();
    let reviewer_decision = Uuid::new_v4();
    let screening_event = Uuid::new_v4();
    let activity = Uuid::new_v4();
    let second_activity = Uuid::new_v4();
    let batch = Uuid::new_v4();
    let acquisition = Uuid::new_v4();
    let cohort = Uuid::new_v4();

    sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'A report')")
        .bind(report)
        .execute(&h.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(a_project)
        .bind(report)
        .execute(&h.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO studies (id,project_id,title) VALUES ($1,$2,'A study')")
        .bind(study)
        .bind(a_project)
        .execute(&h.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO protocol_versions (id,project_id,version,name,status,published_at)
         VALUES ($1,$2,1,'Test protocol','published',now())",
    )
    .bind(protocol)
    .bind(a_project)
    .execute(&h.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO acquisition_runs
         (id,project_id,source,strategy,config,metadata,status,max_depth,seed_count,
          queued_count,fetched_count,failed_count,metadata_provider,citation_provider,created_at)
         VALUES ($1,$2,'crossref','citation_traversal','{}'::jsonb,'{}'::jsonb,'queued',2,1,1,0,0,
                 'crossref','crossref',now())",
    )
    .bind(acquisition)
    .bind(a_project)
    .execute(&h.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO ai_runs
         (id,project_id,task_kind,provider,model,prompt_version,input_hash,output,status,
          profile,model_version,schema_version,prompt_hash,schema_hash,reuse_hash,evidence_refs)
         VALUES ($1,$2,'extraction','fixture-provider','fixture-model','prompt-v1',$3,NULL,
                 'completed','fixture-profile','fixture-model-2026','schema-v1',$4,$5,$6,'[]'::jsonb)",
    )
    .bind(run)
    .bind(a_project)
    .bind("a".repeat(64))
    .bind("b".repeat(64))
    .bind("c".repeat(64))
    .bind("d".repeat(64))
    .execute(&h.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ai_proposals
         (id,project_id,ai_run_id,proposal_type,payload,status,entity_type,entity_id,operation,
          model_run_id,authority_tier,task_kind,target_study_id)
         VALUES ($1,$2,$3,'data_extraction',$5,'pending','study',$4,'data_extraction',
                 $3,'scientific_conclusion','data_extraction',$4)",
    )
    .bind(proposal)
    .bind(a_project)
    .bind(run)
    .bind(study)
    .bind(json!({"study_id": study, "fields": []}))
    .execute(&h.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO ai_reviewer_decisions
         (id,project_id,report_id,stage,decision,rationale,evidence,source)
         VALUES ($1,$2,$3,'title_abstract','include','fixture opinion','[]'::jsonb,'ai')",
    )
    .bind(reviewer_decision)
    .bind(a_project)
    .bind(report)
    .execute(&h.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO screening_events
         (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id,created_at)
         VALUES ($1,$2,$3,'title_abstract','include',$4,'user','reviewer-a',
                 now() - interval '1 minute')",
    )
    .bind(screening_event)
    .bind(a_project)
    .bind(report)
    .bind(protocol)
    .execute(&h.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO screening_state
         (project_id,report_id,title_abstract_status,full_text_status,final_status,revision,
          last_event_id)
         VALUES ($1,$2,'include','not_required','include',1,$3)",
    )
    .bind(a_project)
    .bind(report)
    .bind(screening_event)
    .execute(&h.pool)
    .await
    .unwrap();

    for id in [activity, second_activity] {
        sqlx::query(
            "INSERT INTO ai_activity
             (id,project_id,actor_type,actor_label,actor_kind,actor_id,task,action,summary,
              affected,before_state,after_state,undo_kind,batch_id,evidence)
             VALUES ($1,$2,'ai','Fixture AI','system','fixture-ai','screening','screening_decision',
                     'Fixture activity','[]'::jsonb,'{}'::jsonb,'{}'::jsonb,'extraction_values',$3,
                     '[]'::jsonb)",
        )
        .bind(id)
        .bind(a_project)
        .bind(batch)
        .execute(&h.pool)
        .await
        .unwrap();
    }

    sqlx::query(
        "INSERT INTO ai_screening_cohorts
         (id,project_id,protocol_version_id,semantic_bundle_hash,semantic_identity,policy_version,
          status,target_percent,approved_by)
         VALUES ($1,$2,$3,$4,'{}'::jsonb,1,'open',95,'owner-a')",
    )
    .bind(cohort)
    .bind(a_project)
    .bind(protocol)
    .bind("e".repeat(64))
    .execute(&h.pool)
    .await
    .unwrap();

    Some(World {
        h,
        a,
        b,
        a_project,
        b_project,
        acquisition,
        study,
        proposal,
        reviewer_decision,
        activity,
        second_activity,
        batch,
        cohort,
    })
}

/// Sends `foreign` and `missing` with the same body and asserts the answers are
/// indistinguishable.
async fn assert_missing_equivalent(
    h: &Harness,
    person: &Person,
    method: Method,
    foreign: &str,
    missing: &str,
    body: Option<&str>,
) {
    let (foreign_status, foreign_body) = h.send(person, method.clone(), foreign, body).await;
    let (missing_status, missing_body) = h.send(person, method, missing, body).await;
    assert_eq!(
        foreign_status, missing_status,
        "{foreign}: foreign {foreign_status} != missing {missing_status}"
    );
    assert_eq!(
        String::from_utf8_lossy(&foreign_body),
        String::from_utf8_lossy(&missing_body),
        "{foreign}: body differs from the missing-id answer"
    );
}

/// As the harness `send`, plus an `Idempotency-Key`, which the refresh route
/// checks before it looks the acquisition up.
async fn send_refresh(h: &Harness, person: &Person, path: &str) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(
            header::COOKIE,
            format!("{SESSION_COOKIE}={}", person.cookie),
        )
        .header(header::ORIGIN, ORIGIN)
        .header("x-correlation-id", CORRELATION)
        .header("idempotency-key", "cross-tenant-refresh")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let response = h.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, body.to_vec())
}

#[tokio::test]
async fn acquisitions_are_scoped_by_project() {
    let Some(w) = world().await else {
        return;
    };

    // A reaches the run and its list, so the fixture is live.
    let (status, body) =
        w.h.get(
            &w.a,
            &format!("/projects/{}/acquisitions/{}", w.a_project, w.acquisition),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let (status, listed) =
        w.h.json(
            &w.a,
            Method::GET,
            &format!("/projects/{}/acquisitions", w.a_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1, "{listed}");

    // B's own project list does not include A's run.
    let (status, listed) =
        w.h.json(
            &w.b,
            Method::GET,
            &format!("/projects/{}/acquisitions", w.b_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed["items"].as_array().unwrap().is_empty(), "{listed}");

    let foreign = format!("/projects/{}/acquisitions/{}", w.b_project, w.acquisition);
    let missing = format!("/projects/{}/acquisitions/{}", w.b_project, Uuid::new_v4());
    assert_missing_equivalent(&w.h, &w.b, Method::GET, &foreign, &missing, None).await;

    let foreign_items = format!(
        "/projects/{}/acquisitions/{}/items",
        w.b_project, w.acquisition
    );
    let missing_items = format!(
        "/projects/{}/acquisitions/{}/items",
        w.b_project,
        Uuid::new_v4()
    );
    assert_missing_equivalent(
        &w.h,
        &w.b,
        Method::GET,
        &foreign_items,
        &missing_items,
        None,
    )
    .await;

    let foreign_cancel = format!(
        "/projects/{}/acquisitions/{}/cancel",
        w.b_project, w.acquisition
    );
    let missing_cancel = format!(
        "/projects/{}/acquisitions/{}/cancel",
        w.b_project,
        Uuid::new_v4()
    );
    assert_missing_equivalent(
        &w.h,
        &w.b,
        Method::POST,
        &foreign_cancel,
        &missing_cancel,
        None,
    )
    .await;

    let foreign_refresh = format!(
        "/projects/{}/acquisitions/{}/refresh",
        w.b_project, w.acquisition
    );
    let missing_refresh = format!(
        "/projects/{}/acquisitions/{}/refresh",
        w.b_project,
        Uuid::new_v4()
    );
    let (foreign_status, foreign_body) = send_refresh(&w.h, &w.b, &foreign_refresh).await;
    let (missing_status, missing_body) = send_refresh(&w.h, &w.b, &missing_refresh).await;
    assert_eq!(foreign_status, missing_status);
    assert_eq!(
        String::from_utf8_lossy(&foreign_body),
        String::from_utf8_lossy(&missing_body)
    );

    // The denied mutations left the run exactly as it was.
    let status: String = sqlx::query_scalar("SELECT status FROM acquisition_runs WHERE id=$1")
        .bind(w.acquisition)
        .fetch_one(&w.h.pool)
        .await
        .unwrap();
    assert_eq!(status, "queued");
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM acquisition_runs WHERE project_id=$1")
        .bind(w.a_project)
        .fetch_one(&w.h.pool)
        .await
        .unwrap();
    assert_eq!(runs, 1, "B's refresh created no run in A's project");
    let b_runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM acquisition_runs WHERE project_id=$1")
            .bind(w.b_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(
        b_runs, 0,
        "B's refresh created nothing in B's project either"
    );
}

#[tokio::test]
async fn ai_proposals_and_reviewer_decisions_are_scoped_by_project() {
    let Some(w) = world().await else {
        return;
    };

    // A sees the proposal and the reviewer decision.
    let (status, proposal) =
        w.h.json(
            &w.a,
            Method::GET,
            &format!("/projects/{}/ai/proposals/{}", w.a_project, w.proposal),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{proposal}");
    assert_eq!(proposal["id"], w.proposal.to_string());
    let (status, decisions) =
        w.h.json(
            &w.a,
            Method::GET,
            &format!("/projects/{}/ai/reviewer-decisions", w.a_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let decisions = decisions.as_array().unwrap();
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert_eq!(decisions[0]["id"], w.reviewer_decision.to_string());

    // A's agreement includes the independent human/AI pair.
    let (status, agreement) =
        w.h.json(
            &w.a,
            Method::GET,
            &format!("/projects/{}/ai/reviewer-agreement", w.a_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let title_abstract = agreement
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["stage"] == "title_abstract")
        .unwrap();
    assert_eq!(title_abstract["compared"], 1, "{agreement}");

    // B's list and filters see nothing from A.
    let (status, listed) =
        w.h.json(
            &w.b,
            Method::GET,
            &format!("/projects/{}/ai/proposals", w.b_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed["items"].as_array().unwrap().is_empty(), "{listed}");
    let foreign_filter = format!(
        "/projects/{}/ai/proposals?target_study_id={}",
        w.b_project, w.study
    );
    let missing_filter = format!(
        "/projects/{}/ai/proposals?target_study_id={}",
        w.b_project,
        Uuid::new_v4()
    );
    let (foreign_status, foreign_body) = w.h.send(&w.b, Method::GET, &foreign_filter, None).await;
    let (missing_status, missing_body) = w.h.send(&w.b, Method::GET, &missing_filter, None).await;
    assert_eq!(foreign_status, missing_status);
    assert_eq!(foreign_body, missing_body);

    let (status, agreement) =
        w.h.json(
            &w.b,
            Method::GET,
            &format!("/projects/{}/ai/reviewer-agreement", w.b_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        agreement
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["compared"] == 0),
        "B sees A's agreement: {agreement}"
    );
    let (status, decisions) =
        w.h.json(
            &w.b,
            Method::GET,
            &format!("/projects/{}/ai/reviewer-decisions", w.b_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(decisions.as_array().unwrap().is_empty(), "{decisions}");

    let foreign_proposal = format!("/projects/{}/ai/proposals/{}", w.b_project, w.proposal);
    let missing_proposal = format!("/projects/{}/ai/proposals/{}", w.b_project, Uuid::new_v4());
    assert_missing_equivalent(
        &w.h,
        &w.b,
        Method::GET,
        &foreign_proposal,
        &missing_proposal,
        None,
    )
    .await;

    // The decision body also carries A's study id. Even with a well-formed
    // body the proposal must be refused, and remain pending for A.
    let decision_body = json!({
        "decision": "accept",
        "reason": "reviewed the suggestion",
        "reviewed_payload": {
            "kind": "data_extraction",
            "study_id": w.study,
            "fields": []
        }
    })
    .to_string();
    let foreign_decision = format!(
        "/projects/{}/ai/proposals/{}/decision",
        w.b_project, w.proposal
    );
    let missing_decision = format!(
        "/projects/{}/ai/proposals/{}/decision",
        w.b_project,
        Uuid::new_v4()
    );
    assert_missing_equivalent(
        &w.h,
        &w.b,
        Method::POST,
        &foreign_decision,
        &missing_decision,
        Some(&decision_body),
    )
    .await;
    let status: String = sqlx::query_scalar("SELECT status FROM ai_proposals WHERE id=$1")
        .bind(w.proposal)
        .fetch_one(&w.h.pool)
        .await
        .unwrap();
    assert_eq!(status, "pending", "B decided A's proposal");

    let resolve_body = json!({"decision": "include"}).to_string();
    let foreign_resolve = format!(
        "/projects/{}/ai/reviewer-decisions/{}/resolve",
        w.b_project, w.reviewer_decision
    );
    let missing_resolve = format!(
        "/projects/{}/ai/reviewer-decisions/{}/resolve",
        w.b_project,
        Uuid::new_v4()
    );
    assert_missing_equivalent(
        &w.h,
        &w.b,
        Method::POST,
        &foreign_resolve,
        &missing_resolve,
        Some(&resolve_body),
    )
    .await;
    let resolved: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT resolved_at FROM ai_reviewer_decisions WHERE id=$1")
            .bind(w.reviewer_decision)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert!(resolved.is_none(), "B resolved A's reviewer decision");

    // Body-carried child id: B owns the proposal and the project in the path,
    // but the reviewed payload names A's study. It must be refused, and A's
    // project must keep no extraction value from it.
    let b_study = Uuid::new_v4();
    let b_run = Uuid::new_v4();
    let b_proposal = Uuid::new_v4();
    sqlx::query("INSERT INTO studies (id,project_id,title) VALUES ($1,$2,'B study')")
        .bind(b_study)
        .bind(w.b_project)
        .execute(&w.h.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ai_runs
         (id,project_id,task_kind,provider,model,prompt_version,input_hash,output,status,
          profile,model_version,schema_version,prompt_hash,schema_hash,reuse_hash,evidence_refs)
         VALUES ($1,$2,'extraction','b-provider','b-model','b-prompt',$3,NULL,'completed',
                 'b-profile','b-model-2026','b-schema',$4,$5,$6,'[]'::jsonb)",
    )
    .bind(b_run)
    .bind(w.b_project)
    .bind("f".repeat(64))
    .bind("1".repeat(64))
    .bind("2".repeat(64))
    .bind("3".repeat(64))
    .execute(&w.h.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ai_proposals
         (id,project_id,ai_run_id,proposal_type,payload,status,entity_type,entity_id,operation,
          model_run_id,authority_tier,task_kind,target_study_id)
         VALUES ($1,$2,$3,'data_extraction',$5,'pending','study',$4,'data_extraction',
                 $3,'scientific_conclusion','data_extraction',$4)",
    )
    .bind(b_proposal)
    .bind(w.b_project)
    .bind(b_run)
    .bind(b_study)
    .bind(json!({"study_id": b_study, "fields": []}))
    .execute(&w.h.pool)
    .await
    .unwrap();

    let body_with = |study: Uuid| {
        json!({
            "decision": "accept",
            "reason": "reviewed the suggestion",
            "reviewed_payload": {
                "kind": "data_extraction",
                "study_id": study,
                "fields": []
            }
        })
        .to_string()
    };
    let foreign_body = body_with(w.study);
    let missing_body = body_with(Uuid::new_v4());
    let b_decision = format!(
        "/projects/{}/ai/proposals/{}/decision",
        w.b_project, b_proposal
    );
    let (foreign_status, foreign_answer) =
        w.h.json(&w.b, Method::POST, &b_decision, &foreign_body)
            .await;
    let (missing_status, missing_answer) =
        w.h.json(&w.b, Method::POST, &b_decision, &missing_body)
            .await;
    assert_eq!(foreign_status, StatusCode::BAD_REQUEST, "{foreign_answer}");
    assert_eq!(foreign_status, missing_status);
    assert_eq!(foreign_answer, missing_answer);
    let status: String = sqlx::query_scalar("SELECT status FROM ai_proposals WHERE id=$1")
        .bind(b_proposal)
        .fetch_one(&w.h.pool)
        .await
        .unwrap();
    assert_eq!(status, "pending", "B's own proposal was decided");
    let values: i64 =
        sqlx::query_scalar("SELECT count(*) FROM extraction_values WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(values, 0, "the foreign body id wrote into A's project");
}

#[tokio::test]
async fn ai_activity_and_batches_are_scoped_by_project() {
    let Some(w) = world().await else {
        return;
    };

    let (status, listed) =
        w.h.json(
            &w.a,
            Method::GET,
            &format!("/projects/{}/ai/activity", w.a_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 2, "{listed}");

    let (status, listed) =
        w.h.json(
            &w.b,
            Method::GET,
            &format!("/projects/{}/ai/activity", w.b_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed["items"].as_array().unwrap().is_empty(), "{listed}");

    // The batch filter must not reach into A's project.
    let foreign_batch = format!("/projects/{}/ai/activity?batch_id={}", w.b_project, w.batch);
    let missing_batch = format!(
        "/projects/{}/ai/activity?batch_id={}",
        w.b_project,
        Uuid::new_v4()
    );
    let (foreign_status, foreign_body) = w.h.send(&w.b, Method::GET, &foreign_batch, None).await;
    let (missing_status, missing_body) = w.h.send(&w.b, Method::GET, &missing_batch, None).await;
    assert_eq!(foreign_status, missing_status);
    assert_eq!(foreign_body, missing_body);
    let (status, own_batch) =
        w.h.json(
            &w.a,
            Method::GET,
            &format!("/projects/{}/ai/activity?batch_id={}", w.a_project, w.batch),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(own_batch["items"].as_array().unwrap().len(), 2);

    let (status, overview) =
        w.h.json(
            &w.b,
            Method::GET,
            &format!("/projects/{}/ai/activity/overview", w.b_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overview["undoable"], 0, "{overview}");
    assert_eq!(overview["to_verify"], 0, "{overview}");
    let (status, overview) =
        w.h.json(
            &w.a,
            Method::GET,
            &format!("/projects/{}/ai/activity/overview", w.a_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overview["undoable"], 2, "{overview}");

    let foreign_undo = format!("/projects/{}/ai/activity/{}/undo", w.b_project, w.activity);
    let missing_undo = format!(
        "/projects/{}/ai/activity/{}/undo",
        w.b_project,
        Uuid::new_v4()
    );
    assert_missing_equivalent(&w.h, &w.b, Method::POST, &foreign_undo, &missing_undo, None).await;

    let foreign_batch_undo = format!(
        "/projects/{}/ai/activity/batches/{}/undo",
        w.b_project, w.batch
    );
    let missing_batch_undo = format!(
        "/projects/{}/ai/activity/batches/{}/undo",
        w.b_project,
        Uuid::new_v4()
    );
    assert_missing_equivalent(
        &w.h,
        &w.b,
        Method::POST,
        &foreign_batch_undo,
        &missing_batch_undo,
        None,
    )
    .await;

    let undone: Vec<Option<chrono::DateTime<chrono::Utc>>> =
        sqlx::query_scalar("SELECT undone_at FROM ai_activity WHERE id=ANY($1) ORDER BY id")
            .bind(vec![w.activity, w.second_activity])
            .fetch_all(&w.h.pool)
            .await
            .unwrap();
    assert!(
        undone.iter().all(Option::is_none),
        "B undid A's activity: {undone:?}"
    );

    // A can still undo its own batch, so the refusal above was authorization,
    // not a broken fixture.
    let (status, body) =
        w.h.json(
            &w.a,
            Method::POST,
            &format!(
                "/projects/{}/ai/activity/batches/{}/undo",
                w.a_project, w.batch
            ),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["undone"], 2, "{body}");
}

#[tokio::test]
async fn ai_first_cohorts_are_scoped_by_project() {
    let Some(w) = world().await else {
        return;
    };

    let (status, overview) =
        w.h.json(
            &w.a,
            Method::GET,
            &format!("/projects/{}/ai/first", w.a_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let cohorts = overview["cohorts"].as_array().unwrap();
    assert_eq!(cohorts.len(), 1, "{overview}");
    assert_eq!(cohorts[0]["id"], w.cohort.to_string());

    let (status, overview) =
        w.h.json(
            &w.b,
            Method::GET,
            &format!("/projects/{}/ai/first", w.b_project),
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        overview["cohorts"].as_array().unwrap().is_empty(),
        "B sees A's cohort: {overview}"
    );

    let actions = [
        ("close", "{}"),
        ("draw", "{}"),
        ("evaluate", "{}"),
        ("finalize", r#"{"acknowledge_reference_limits":true}"#),
        ("recover", "{}"),
    ];
    for (action, body) in actions {
        let foreign = format!("/projects/{}/ai/first/{}/{}", w.b_project, w.cohort, action);
        let missing = format!(
            "/projects/{}/ai/first/{}/{}",
            w.b_project,
            Uuid::new_v4(),
            action
        );
        assert_missing_equivalent(&w.h, &w.b, Method::POST, &foreign, &missing, Some(body)).await;
    }

    // Starting a cohort in B's project does not touch A's.
    let (status, body) =
        w.h.json(
            &w.b,
            Method::POST,
            &format!("/projects/{}/ai/first", w.b_project),
            r#"{"target_percent":95,"allow_finalization":true}"#,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "ai_first_refused", "{body}");

    let status: String = sqlx::query_scalar("SELECT status FROM ai_screening_cohorts WHERE id=$1")
        .bind(w.cohort)
        .fetch_one(&w.h.pool)
        .await
        .unwrap();
    assert_eq!(status, "open", "B changed A's cohort");
    let cohorts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_screening_cohorts WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(cohorts, 1, "B started a cohort in A's project");

    // No stale authority row was written for A by B's start attempt.
    let authority: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_ai_screening_authority WHERE project_id=$1",
    )
    .bind(w.a_project)
    .fetch_one(&w.h.pool)
    .await
    .unwrap();
    assert_eq!(authority, 0);
}
