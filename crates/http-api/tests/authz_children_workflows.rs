#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! Cross-workspace child access for workflows, automation runs and downloads
//! (handoff 03b).
//!
//! Workspace B owns project PB. Workspace A owns PA, which holds a published
//! workflow with a run, an automation definition with a run, a workflow file,
//! and a report with a document. B calls each child route under PB with A's
//! child id. The answer must equal the answer for a random id, and none of A's
//! rows may change.
//!
//! Scoping of the handler SQL that these tests rely on:
//! - `crates/app/src/workflows.rs`: `get_workflow`, `set_workflow_enabled`,
//!   `update_workflow`, `delete_workflow`, `publish_workflow`,
//!   `rotate_endpoint_credentials`, `get_webhook_config`,
//!   `set_webhook_signature_required`, `cancel_workflow_run`, `get_workflow_run`,
//!   `list_workflow_runs`, `get_workflow_file` all filter `project_id`.
//!   `list_workflow_versions` and `get_workflow_version` call `get_workflow`
//!   first. `get_published_graph`, `get_workflow_secret` and
//!   `start_run_in_transaction` select by workflow id only and are reached
//!   after a project-scoped `get_workflow`.
//! - `crates/app/src/automations.rs`: the manual-start path, the idempotency
//!   lookup and `get_automation_run` filter `project_id`.
//! - `crates/app/src/documents.rs`: `get_document`, `delete_document`,
//!   `acknowledge_identity_mismatch` filter project, report and document id.
//! - `crates/http-api/src/routes/documents.rs`: `ensure_report_scope` checks
//!   `project_reports` before every report-scoped handler.

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use deepref_app::NewDocument;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

mod support;
use support::authz::{CORRELATION, Harness, Person};
use support::{ORIGIN, SESSION_COOKIE};

/// The child ids the calls in this file name. `own_report` is a report of B's
/// own project, so it is the same on both sides of every comparison.
#[derive(Clone, Copy)]
struct Ids {
    workflow: Uuid,
    run: Uuid,
    file: Uuid,
    definition: Uuid,
    auto_run: Uuid,
    report: Uuid,
    document: Uuid,
    own_report: Uuid,
}

impl Ids {
    /// Fresh random ids, except the report of B's own project.
    fn random(own_report: Uuid) -> Self {
        Self {
            workflow: Uuid::new_v4(),
            run: Uuid::new_v4(),
            file: Uuid::new_v4(),
            definition: Uuid::new_v4(),
            auto_run: Uuid::new_v4(),
            report: Uuid::new_v4(),
            document: Uuid::new_v4(),
            own_report,
        }
    }
}

struct World {
    h: Harness,
    a: Person,
    b: Person,
    pa: Uuid,
    pb: Uuid,
    /// A's children, the ids B will try to use under PB.
    ids: Ids,
}

/// One request, sent as a principal, with what to check about its answer.
struct Call {
    label: &'static str,
    method: Method,
    path: String,
    headers: Vec<(&'static str, String)>,
    body: Option<(&'static str, Vec<u8>)>,
    /// Whether the answer must be 404. A list filtered by a foreign id answers
    /// 200 with nothing in it, so it is only compared with its random twin.
    expect_not_found: bool,
}

impl Call {
    fn new(method: Method, label: &'static str, path: String) -> Self {
        Self {
            label,
            method,
            path,
            headers: Vec::new(),
            body: None,
            expect_not_found: true,
        }
    }

    fn get(label: &'static str, path: String) -> Self {
        Self::new(Method::GET, label, path)
    }

    fn json(method: Method, label: &'static str, path: String, body: &str) -> Self {
        Self::new(method, label, path).body("application/json", body.as_bytes().to_vec())
    }

    fn body(mut self, content_type: &'static str, bytes: Vec<u8>) -> Self {
        self.body = Some((content_type, bytes));
        self
    }

    fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }

    fn same_only(mut self) -> Self {
        self.expect_not_found = false;
        self
    }
}

async fn send(h: &Harness, who: &Person, call: &Call) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(call.method.clone())
        .uri(call.path.as_str())
        .header(header::COOKIE, format!("{SESSION_COOKIE}={}", who.cookie))
        .header(header::ORIGIN, ORIGIN)
        .header("x-correlation-id", CORRELATION);
    for (name, value) in &call.headers {
        request = request.header(*name, value.as_str());
    }
    let payload = match &call.body {
        Some((content_type, bytes)) => {
            request = request.header(header::CONTENT_TYPE, *content_type);
            bytes.clone()
        }
        None => Vec::new(),
    };
    let response = h
        .app
        .clone()
        .oneshot(request.body(Body::from(payload)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn uuid_at(value: &Value) -> Uuid {
    Uuid::parse_str(value.as_str().expect("an id string")).unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

async fn insert_report(pool: &PgPool, project: Uuid, title: &str) -> Uuid {
    let report = Uuid::new_v4();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,$2)")
        .bind(report)
        .bind(title)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project)
        .bind(report)
        .execute(pool)
        .await
        .unwrap();
    report
}

async fn insert_document(pool: &PgPool, project: Uuid, report: Uuid) -> Uuid {
    let document = Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    deepref_app::create_document(
        &mut tx,
        NewDocument {
            project_id: project,
            report_id: report,
            id: document,
            source: "external_url",
            status: "external",
            original_filename: None,
            external_url: Some("https://example.org/a.pdf"),
            mime_type: "application/pdf",
            byte_size: 0,
            content_hash: None,
            object_key: None,
            actor_kind: "user",
            actor_id: "test-user",
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    document
}

/// A signs in and owns PA with a published workflow and a run, a definition
/// and a run, a workflow file, and a report with a document. B owns PB with a
/// report of its own.
async fn world() -> Option<World> {
    let h = Harness::new().await?;
    let a = h.sign_in("user_A", "a@example.org").await;
    let b = h.sign_in("user_B", "b@example.org").await;
    let pa = h.create_project(&a, "A's project").await;
    let pb = h.create_project(&b, "B's project").await;

    let (status, body) = h
        .json(
            &a,
            Method::POST,
            &format!("/projects/{pa}/automations/workflows/from-template"),
            r#"{"template_id":"project_maintenance","name":"A's flow"}"#,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let workflow = uuid_at(&body["id"]);
    let (status, body) = h
        .json(
            &a,
            Method::POST,
            &format!("/projects/{pa}/automations/workflows/{workflow}/publish"),
            "{}",
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = h
        .json(
            &a,
            Method::POST,
            &format!("/projects/{pa}/automations/workflows/{workflow}/runs"),
            "{}",
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let run = uuid_at(&body["run_id"]);

    let (status, body) = h
        .json(
            &a,
            Method::PUT,
            &format!("/projects/{pa}/automations/definitions/project_maintenance.v1"),
            r#"{"name":"Maintenance","trigger":"manual","status":"active"}"#,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let definition = uuid_at(&body["id"]);
    let (status, bytes) = send(
        &h,
        &a,
        &Call::new(
            Method::POST,
            "automation run",
            format!("/projects/{pa}/automations/runs"),
        )
        .header("idempotency-key", "a-1")
        .body(
            "application/json",
            format!(r#"{{"definition_id":"{definition}"}}"#).into_bytes(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{}", text(&bytes));
    let auto_run = uuid_at(&parse(&bytes)["run_id"]);

    let file = deepref_app::create_workflow_file(
        &h.pool,
        pa,
        run,
        "export",
        "records.csv",
        "text/csv",
        b"id\n1\n",
    )
    .await
    .unwrap();
    let report = insert_report(&h.pool, pa, "A report").await;
    let document = insert_document(&h.pool, pa, report).await;
    let own_report = insert_report(&h.pool, pb, "B report").await;

    Some(World {
        h,
        a,
        b,
        pa,
        pb,
        ids: Ids {
            workflow,
            run,
            file,
            definition,
            auto_run,
            report,
            document,
            own_report,
        },
    })
}

/// Every row of A's that a call under PB could change, and B's own counts.
/// Each entry is a digest of the row or a count, so any change shows.
async fn snapshot(w: &World) -> Vec<(&'static str, String)> {
    let ids = w.ids;
    let rows: Vec<(&'static str, &'static str, Uuid)> = vec![
        (
            "workflow",
            "SELECT COALESCE((SELECT md5(row_to_json(x)::text) FROM workflows x WHERE x.id=$1),'absent')",
            ids.workflow,
        ),
        (
            "workflow versions",
            "SELECT count(*)::text FROM workflow_versions WHERE workflow_id=$1",
            ids.workflow,
        ),
        (
            "runs of the workflow",
            "SELECT count(*)::text FROM workflow_run_details WHERE workflow_id=$1",
            ids.workflow,
        ),
        (
            "workflow run",
            "SELECT COALESCE((SELECT md5(row_to_json(x)::text) FROM durable_runs x WHERE x.id=$1),'absent')",
            ids.run,
        ),
        (
            "workflow run details",
            "SELECT COALESCE((SELECT md5(row_to_json(x)::text) FROM workflow_run_details x WHERE x.id=$1),'absent')",
            ids.run,
        ),
        (
            "workflow file",
            "SELECT count(*)::text FROM workflow_files WHERE id=$1",
            ids.file,
        ),
        (
            "definition",
            "SELECT COALESCE((SELECT md5(row_to_json(x)::text) FROM automation_definitions x WHERE x.id=$1),'absent')",
            ids.definition,
        ),
        (
            "automation run",
            "SELECT COALESCE((SELECT md5(row_to_json(x)::text) FROM durable_runs x WHERE x.id=$1),'absent')",
            ids.auto_run,
        ),
        (
            "automation run details",
            "SELECT COALESCE((SELECT md5(row_to_json(x)::text) FROM automation_run_details x WHERE x.id=$1),'absent')",
            ids.auto_run,
        ),
        (
            "document",
            "SELECT COALESCE((SELECT md5(row_to_json(x)::text) FROM documents x WHERE x.id=$1),'absent')",
            ids.document,
        ),
        (
            "documents of the report",
            "SELECT count(*)::text FROM documents WHERE report_id=$1",
            ids.report,
        ),
        (
            "links of the report",
            "SELECT count(*)::text FROM project_reports WHERE report_id=$1",
            ids.report,
        ),
        (
            "A's runs",
            "SELECT count(*)::text FROM durable_runs WHERE project_id=$1",
            w.pa,
        ),
        (
            "A's automation run details",
            "SELECT count(*)::text FROM automation_run_details WHERE project_id=$1",
            w.pa,
        ),
        (
            "A's workflows",
            "SELECT count(*)::text FROM workflows WHERE project_id=$1",
            w.pa,
        ),
        (
            "A's documents",
            "SELECT count(*)::text FROM documents WHERE project_id=$1",
            w.pa,
        ),
        (
            "B's runs",
            "SELECT count(*)::text FROM durable_runs WHERE project_id=$1",
            w.pb,
        ),
        (
            "B's automation run details",
            "SELECT count(*)::text FROM automation_run_details WHERE project_id=$1",
            w.pb,
        ),
        (
            "B's workflows",
            "SELECT count(*)::text FROM workflows WHERE project_id=$1",
            w.pb,
        ),
        (
            "B's documents",
            "SELECT count(*)::text FROM documents WHERE project_id=$1",
            w.pb,
        ),
        (
            "B's report links",
            "SELECT count(*)::text FROM project_reports WHERE project_id=$1",
            w.pb,
        ),
    ];
    let mut out = Vec::with_capacity(rows.len());
    for (name, sql, id) in rows {
        let value: String = sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
        out.push((name, value));
    }
    out
}

/// Sends each foreign call and its random-id twin as B and returns every
/// failure: a foreign answer that is not the twin's, or any change to A's rows
/// across the pair.
async fn compare(w: &World, foreign: Vec<Call>, missing: Vec<Call>) -> Vec<String> {
    assert_eq!(foreign.len(), missing.len());
    let mut failures = Vec::new();
    for (f, m) in foreign.iter().zip(&missing) {
        assert_eq!(f.label, m.label);
        let before = snapshot(w).await;
        let (fs, fb) = send(&w.h, &w.b, f).await;
        let (ms, mb) = send(&w.h, &w.b, m).await;
        let after = snapshot(w).await;
        if f.expect_not_found && fs != StatusCode::NOT_FOUND {
            failures.push(format!(
                "{}: foreign child answered {fs}: {}",
                f.label,
                text(&fb)
            ));
        }
        if fs != ms || fb != mb {
            failures.push(format!(
                "{}: foreign {fs} {} differs from random {ms} {}",
                f.label,
                text(&fb),
                text(&mb)
            ));
        }
        for ((name, was), (_, now)) in before.iter().zip(&after) {
            if was != now {
                failures.push(format!("{}: A's {name} changed", f.label));
            }
        }
    }
    failures
}

/// Read routes whose child id is in the path or in the query.
fn workflow_reads(p: Uuid, ids: &Ids) -> Vec<Call> {
    let auto = format!("/projects/{p}/automations");
    let wf = ids.workflow;
    vec![
        Call::get("workflow", format!("{auto}/workflows/{wf}")),
        Call::get("versions", format!("{auto}/workflows/{wf}/versions")),
        Call::get("version", format!("{auto}/workflows/{wf}/versions/1")),
        Call::get("runs of workflow", format!("{auto}/workflows/{wf}/runs")),
        Call::get("endpoints", format!("{auto}/workflows/{wf}/endpoints")),
        Call::get("workflow run", format!("{auto}/workflow-runs/{}", ids.run)),
        Call::get(
            "workflow file",
            format!("{auto}/workflow-files/{}", ids.file),
        ),
        Call::get(
            "project runs filtered by workflow",
            format!("{auto}/workflow-runs?workflow_id={wf}"),
        )
        .same_only(),
        Call::get("automation run", format!("{auto}/runs/{}", ids.auto_run)),
    ]
}

/// Article file reads: the document record, its text and its download.
fn document_reads(p: Uuid, ids: &Ids) -> Vec<Call> {
    let report = format!("/projects/{p}/reports/{}", ids.report);
    let own = format!("/projects/{p}/reports/{}", ids.own_report);
    let doc = ids.document;
    vec![
        Call::get("document list of report", format!("{report}/documents")),
        Call::get("document", format!("{report}/documents/{doc}")),
        Call::get(
            "document blocks",
            format!("{report}/documents/{doc}/blocks"),
        ),
        Call::get("document pages", format!("{report}/documents/{doc}/pages")),
        Call::get(
            "document sections",
            format!("{report}/documents/{doc}/sections"),
        ),
        Call::get(
            "document references",
            format!("{report}/documents/{doc}/references"),
        ),
        Call::get(
            "document content",
            format!("{report}/documents/{doc}/content"),
        ),
        Call::get(
            "document under own report",
            format!("{own}/documents/{doc}"),
        ),
        Call::get(
            "document content under own report",
            format!("{own}/documents/{doc}/content"),
        ),
    ]
}

/// Mutating workflow, endpoint, run and automation-run routes.
fn workflow_mutations(p: Uuid, ids: &Ids) -> Vec<Call> {
    let auto = format!("/projects/{p}/automations");
    let wf = format!("{auto}/workflows/{}", ids.workflow);
    let run = format!("{auto}/workflow-runs/{}", ids.run);
    vec![
        Call::json(
            Method::PUT,
            "update name",
            wf.clone(),
            r#"{"name":"Hijacked"}"#,
        ),
        Call::json(
            Method::PUT,
            "replace graph",
            wf.clone(),
            r#"{"graph":{"nodes":[],"edges":[]}}"#,
        ),
        Call::new(Method::DELETE, "delete workflow", wf.clone()),
        Call::json(Method::POST, "validate", format!("{wf}/validate"), "{}"),
        Call::json(
            Method::POST,
            "publish",
            format!("{wf}/publish"),
            r#"{"note":"hijacked"}"#,
        ),
        Call::json(Method::POST, "enable", format!("{wf}/enable"), "{}"),
        Call::json(Method::POST, "disable", format!("{wf}/disable"), "{}"),
        Call::json(Method::POST, "start run", format!("{wf}/runs"), "{}"),
        Call::json(
            Method::POST,
            "start test run",
            format!("{wf}/test-runs"),
            "{}",
        ),
        Call::json(
            Method::PUT,
            "signature requirement",
            format!("{wf}/endpoints"),
            r#"{"signature_required":false}"#,
        ),
        Call::json(
            Method::POST,
            "rotate webhook",
            format!("{wf}/endpoints/webhook/rotate"),
            "{}",
        ),
        Call::json(
            Method::POST,
            "rotate webhook secret",
            format!("{wf}/endpoints/webhook-secret/rotate"),
            "{}",
        ),
        Call::json(
            Method::POST,
            "rotate email address",
            format!("{wf}/endpoints/email/rotate"),
            "{}",
        ),
        Call::json(Method::POST, "cancel run", format!("{run}/cancel"), "{}"),
        // The same Idempotency-Key as A's run: a lookup that is not scoped to
        // the project would replay A's run here.
        Call::json(
            Method::POST,
            "automation run of a foreign definition",
            format!("{auto}/runs"),
            &format!(r#"{{"definition_id":"{}"}}"#, ids.definition),
        )
        .header("idempotency-key", "a-1"),
    ]
}

/// Mutating article file routes, with the report and document in the path.
fn document_mutations(p: Uuid, ids: &Ids) -> Vec<Call> {
    let report = format!("/projects/{p}/reports/{}", ids.report);
    let own = format!("/projects/{p}/reports/{}", ids.own_report);
    let doc = ids.document;
    let pdf_boundary = "multipart/form-data; boundary=deepref-test";
    let pdf = b"--deepref-test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"b.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.4\r\n--deepref-test--\r\n".to_vec();
    vec![
        Call::json(
            Method::POST,
            "reparse",
            format!("{report}/documents/{doc}/reparse"),
            "{}",
        ),
        Call::json(
            Method::POST,
            "acknowledge identity",
            format!("{report}/documents/{doc}/identity/acknowledge"),
            "{}",
        ),
        Call::new(
            Method::DELETE,
            "delete document",
            format!("{report}/documents/{doc}"),
        ),
        Call::json(
            Method::POST,
            "attach external document",
            format!("{report}/documents/external"),
            r#"{"url":"https://example.org/b.pdf"}"#,
        ),
        Call::json(
            Method::POST,
            "attach open-access document",
            format!("{report}/documents/open-access"),
            "{}",
        ),
        Call::new(
            Method::POST,
            "upload document",
            format!("{report}/documents"),
        )
        .body(pdf_boundary, pdf),
        Call::json(
            Method::POST,
            "reparse under own report",
            format!("{own}/documents/{doc}/reparse"),
            "{}",
        ),
        Call::json(
            Method::POST,
            "acknowledge under own report",
            format!("{own}/documents/{doc}/identity/acknowledge"),
            "{}",
        ),
        Call::new(
            Method::DELETE,
            "delete under own report",
            format!("{own}/documents/{doc}"),
        ),
    ]
}

/// Owner controls: the children are real and A still reaches them, so the
/// denials above are not a broken route.
async fn owner_reaches_its_children(w: &World) {
    let auto = format!("/projects/{}/automations", w.pa);
    let ids = w.ids;
    let (status, body) =
        w.h.get(&w.a, &format!("{auto}/workflows/{}", ids.workflow))
            .await;
    assert_eq!(status, StatusCode::OK, "{}", text(&body));
    let (status, _) =
        w.h.get(&w.a, &format!("{auto}/workflow-runs/{}", ids.run))
            .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) =
        w.h.get(&w.a, &format!("{auto}/workflow-files/{}", ids.file))
            .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"id\n1\n", "the file downloads its content");
    let (status, _) =
        w.h.get(&w.a, &format!("{auto}/runs/{}", ids.auto_run))
            .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) =
        w.h.get(
            &w.a,
            &format!(
                "/projects/{}/reports/{}/documents/{}",
                w.pa, ids.report, ids.document
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) =
        w.h.get(
            &w.b,
            &format!("/projects/{}/reports/{}/documents", w.pb, ids.own_report),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", text(&body));
    assert_eq!(parse(&body), json!([]));
}

#[tokio::test]
async fn workflow_reads_under_a_foreign_project_answer_like_a_random_child() {
    let Some(w) = world().await else {
        return;
    };
    let missing = Ids::random(w.ids.own_report);
    let failures = compare(
        &w,
        workflow_reads(w.pb, &w.ids),
        workflow_reads(w.pb, &missing),
    )
    .await;
    owner_reaches_its_children(&w).await;
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn workflow_mutations_under_a_foreign_project_are_refused_and_change_nothing() {
    let Some(w) = world().await else {
        return;
    };
    let missing = Ids::random(w.ids.own_report);
    let failures = compare(
        &w,
        workflow_mutations(w.pb, &w.ids),
        workflow_mutations(w.pb, &missing),
    )
    .await;
    // The workflow is still A's, and still published and running.
    let (status, body) =
        w.h.get(
            &w.a,
            &format!(
                "/projects/{}/automations/workflows/{}",
                w.pa, w.ids.workflow
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", text(&body));
    assert_eq!(parse(&body)["name"], "A's flow");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn a_foreign_definition_is_not_found_and_its_key_does_not_replay_a_run() {
    let Some(w) = world().await else {
        return;
    };
    let auto = format!("/projects/{}/automations", w.pb);
    let foreign_body = format!(r#"{{"definition_id":"{}"}}"#, w.ids.definition);
    for key in ["a-1", "fresh-key"] {
        let (status, bytes) = send(
            &w.h,
            &w.b,
            &Call::new(Method::POST, "start", format!("{auto}/runs"))
                .header("idempotency-key", key)
                .body("application/json", foreign_body.clone().into_bytes()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{key}: {}", text(&bytes));
        assert!(
            !text(&bytes).contains(&w.ids.auto_run.to_string()),
            "{key}: A's run id must not be replayed: {}",
            text(&bytes)
        );
    }
    let (status, body) = w.h.get(&w.b, &format!("{auto}/runs")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(parse(&body), json!([]), "B's run list stays empty");
    let own = format!("/projects/{}/automations", w.pa);
    let (status, body) = w.h.get(&w.a, &format!("{own}/runs")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        parse(&body).as_array().map(Vec::len),
        Some(1),
        "A's own automation run list still holds exactly A's run: {}",
        text(&body)
    );
}

#[tokio::test]
async fn article_file_reads_and_downloads_under_a_foreign_project_answer_like_a_random_child() {
    let Some(w) = world().await else {
        return;
    };
    let missing = Ids::random(w.ids.own_report);
    let failures = compare(
        &w,
        document_reads(w.pb, &w.ids),
        document_reads(w.pb, &missing),
    )
    .await;
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn article_file_mutations_under_a_foreign_project_are_refused_and_change_nothing() {
    let Some(w) = world().await else {
        return;
    };
    let missing = Ids::random(w.ids.own_report);
    let failures = compare(
        &w,
        document_mutations(w.pb, &w.ids),
        document_mutations(w.pb, &missing),
    )
    .await;
    let (status, body) =
        w.h.get(
            &w.a,
            &format!(
                "/projects/{}/reports/{}/documents/{}",
                w.pa, w.ids.report, w.ids.document
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", text(&body));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn exports_and_workflow_downloads_are_scoped_to_the_project_in_the_path() {
    let Some(w) = world().await else {
        return;
    };
    let kinds = [
        "reports.csv",
        "reports.json",
        "reports.ris",
        "reports.bib",
        "extraction.csv",
        "appraisal.csv",
        "included_studies.csv",
        "prisma.json",
        "prisma.svg",
        "audit.csv",
        "protocol.json",
    ];
    let random_project = Uuid::new_v4();
    let mut failures = Vec::new();
    for kind in kinds {
        let foreign = Call::get(
            "export of a foreign project",
            format!("/projects/{}/exports/{kind}", w.pa),
        );
        let missing = Call::get(
            "export of a missing project",
            format!("/projects/{random_project}/exports/{kind}"),
        );
        let (fs, fb) = send(&w.h, &w.b, &foreign).await;
        let (ms, mb) = send(&w.h, &w.b, &missing).await;
        if fs != StatusCode::NOT_FOUND || fs != ms || fb != mb {
            failures.push(format!(
                "{kind}: foreign {fs} {} differs from missing {ms} {}",
                text(&fb),
                text(&mb)
            ));
        }
    }
    // B's own export holds none of A's records.
    let (status, body) =
        w.h.get(&w.b, &format!("/projects/{}/exports/reports.csv", w.pb))
            .await;
    assert_eq!(status, StatusCode::OK, "{}", text(&body));
    assert!(!text(&body).contains("A report"), "{}", text(&body));
    // The owner's export does hold A's record: the control for the check above.
    let (status, body) =
        w.h.get(&w.a, &format!("/projects/{}/exports/reports.csv", w.pa))
            .await;
    assert_eq!(status, StatusCode::OK, "{}", text(&body));
    assert!(text(&body).contains("A report"), "{}", text(&body));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn graph_ids_in_a_foreign_body_are_not_resolved_and_change_nothing() {
    let Some(w) = world().await else {
        return;
    };
    let (status, body) =
        w.h.get(
            &w.a,
            &format!(
                "/projects/{}/automations/workflows/{}",
                w.pa, w.ids.workflow
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", text(&body));
    let draft = parse(&body)["graph"].clone();

    // The same draft with A's report and A's workflow id in a block's config,
    // and with random ids in the same places.
    let with_ids = |report: Uuid, workflow: Uuid| {
        let mut graph = draft.clone();
        for node in graph["nodes"].as_array_mut().unwrap() {
            if node["id"] == "refresh" {
                node["config"] = json!({ "report_id": report, "workflow_id": workflow });
            }
        }
        graph
    };
    let foreign_graph = with_ids(w.ids.report, w.ids.workflow);
    let random_graph = with_ids(Uuid::new_v4(), Uuid::new_v4());

    // The block details read the graph in the body and nothing from the database.
    let details = |graph: &Value| {
        Call::json(
            Method::POST,
            "block details",
            format!("/projects/{}/automations/block-details", w.pb),
            &json!({ "graph": graph, "node_id": "refresh" }).to_string(),
        )
    };
    let (fs, fb) = send(&w.h, &w.b, &details(&foreign_graph)).await;
    let (ms, mb) = send(&w.h, &w.b, &details(&random_graph)).await;
    assert_eq!(fs, StatusCode::OK, "{}", text(&fb));
    assert_eq!(
        (fs, fb),
        (ms, mb),
        "block details read nothing from the database"
    );

    // Saving a draft in B's project with A's ids: the ids are stored as text
    // and nothing of A's is returned or changed.
    let before = snapshot(&w).await;
    let create = Call::json(
        Method::POST,
        "create with foreign ids",
        format!("/projects/{}/automations/workflows", w.pb),
        &json!({ "name": "B's copy", "graph": foreign_graph }).to_string(),
    );
    let (status, bytes) = send(&w.h, &w.b, &create).await;
    if status == StatusCode::CREATED {
        let created = uuid_at(&parse(&bytes)["id"]);
        let (status, body) =
            w.h.get(
                &w.b,
                &format!("/projects/{}/automations/workflows/{created}", w.pb),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{}", text(&body));
        assert!(
            !text(&body).contains("A report"),
            "A's data is not shown to B"
        );
    }
    let after = snapshot(&w).await;
    // B's own counts may grow by the copy; A's rows must not move.
    let changed: Vec<&str> = before
        .iter()
        .zip(&after)
        .filter(|(was, now)| was.1 != now.1 && !was.0.starts_with("B's"))
        .map(|(was, _)| was.0)
        .collect();
    assert!(changed.is_empty(), "A's rows changed: {changed:?}");
}
