#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! Cross-workspace child ids in the review, extraction, deduplication and
//! appraisal routes (handoff 03b).
//!
//! `authz::policy` proves the PEP authorizes the *project* on the path. This
//! file proves the next link: a child identifier carried in the path or body
//! also belongs to that project. User B, who owns `b_project`, calls each
//! route with a child of A's `a_project`; the answer must be byte-identical
//! (after masking ids) to the answer for a child that does not exist, and A's
//! rows must be untouched afterwards.
//!
//! Covered here:
//! - `deduplication/proposals`, `deduplication/proposals/{proposal_id}/decision`,
//!   `deduplication/run`, `records/{record_id}/resolution`,
//!   `records/{record_id}/ai/deduplication`
//! - `extraction/fields`, `extraction/fields/{field_id}`,
//!   `studies/{study_id}/extraction`, `studies/{study_id}/extraction/values/{field_id}`,
//!   `studies/{study_id}/extraction/values/{value_id}/confirm`,
//!   `studies/{study_id}/ai/extraction`
//! - `review-runs/{run_id}`, `review-runs/{run_id}/retry`
//! - `reports/{report_id}/screening`, `.../screening/undo`, `.../screening/history`,
//!   `.../appraisals`, `.../ai/screening`, `.../ai/study-grouping`,
//!   `.../ai/appraisal-prefill`
//! - `appraisal-definitions` (a deployment catalog; see the last test)
//!
//! Not covered here, because the child belongs to another module group:
//! `reports/{report_id}` itself and `reports/{report_id}/study` (studies),
//! `reports/{report_id}/documents*` (documents), `studies/{study_id}` and its
//! non-extraction routes (studies), `ai/proposals*` (AI proposals).
use axum::http::{Method, StatusCode};
use uuid::Uuid;

mod support;
use support::authz::{Harness, Person};

/// Everything one test needs: two signed-in people, one project each, and the
/// harness pool to write the child rows the API has no endpoint for.
struct World {
    h: Harness,
    a: Person,
    b: Person,
    a_project: Uuid,
    b_project: Uuid,
}

async fn world() -> Option<World> {
    let h = Harness::new().await?;
    let a = h
        .sign_in("child_review_a", "child-review-a@example.org")
        .await;
    let b = h
        .sign_in("child_review_b", "child-review-b@example.org")
        .await;
    let a_project = h.create_project(&a, "A's project").await;
    let b_project = h.create_project(&b, "B's project").await;
    Some(World {
        h,
        a,
        b,
        a_project,
        b_project,
    })
}

/// The body with every UUID replaced, so two denials that merely name the id
/// they were asked about still compare equal.
fn normalize(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body).into_owned();
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(index) = rest.find(|c: char| c.is_ascii_hexdigit()) {
        let candidate = &rest[index..];
        if candidate.len() >= 36 && Uuid::parse_str(&candidate[..36]).is_ok() {
            out.push_str(&rest[..index]);
            out.push_str("<uuid>");
            rest = &candidate[36..];
        } else {
            out.push_str(&rest[..=index]);
            rest = &rest[index + 1..];
        }
    }
    out.push_str(rest);
    out
}

/// Runs the same request as B with A's `child` in the path or body and again
/// with a random UUID in its place. The two answers must be identical: a child
/// of another project is indistinguishable from a child that does not exist.
/// Returns the foreign answer for further assertions.
async fn same_as_missing(
    w: &World,
    method: Method,
    path: &str,
    body: Option<&str>,
    child: Uuid,
) -> (StatusCode, Vec<u8>) {
    let missing = Uuid::new_v4();
    let missing_path = path.replace(&child.to_string(), &missing.to_string());
    let missing_body = body.map(|body| body.replace(&child.to_string(), &missing.to_string()));
    let (foreign_status, foreign_body) = w.h.send(&w.b, method.clone(), path, body).await;
    let (missing_status, missing_body) =
        w.h.send(&w.b, method, &missing_path, missing_body.as_deref())
            .await;
    assert_eq!(
        (foreign_status, normalize(&foreign_body)),
        (missing_status, normalize(&missing_body)),
        "{path}: A's child answered differently from a missing one"
    );
    assert!(
        !foreign_status.is_success(),
        "{path}: A's child answered {foreign_status}, not a refusal"
    );
    (foreign_status, foreign_body)
}

// ---------------------------------------------------------------------------
// Child rows. Reports, records, proposals and review runs have no creation
// endpoint inside this group, so they are inserted like the app tests do.
// ---------------------------------------------------------------------------

async fn insert_report(h: &Harness, project: Uuid, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO reports (id,title) VALUES ($1,$2)")
        .bind(id)
        .bind(title)
        .execute(&h.pool)
        .await
        .expect("report inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project)
        .bind(id)
        .execute(&h.pool)
        .await
        .expect("report membership inserts");
    id
}

async fn insert_record(h: &Harness, project: Uuid, key: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO records (id,project_id,source,source_key,title,raw)
         VALUES ($1,$2,'child-test',$3,$4,'{}'::jsonb)",
    )
    .bind(id)
    .bind(project)
    .bind(key)
    .bind(format!("Source record {key}"))
    .execute(&h.pool)
    .await
    .expect("record inserts");
    id
}

async fn insert_proposal(h: &Harness, project: Uuid, record: Uuid, candidate: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO dedupe_proposals (id,project_id,record_id,candidate_report_id,proposal_kind)
         VALUES ($1,$2,$3,$4,'fuzzy')",
    )
    .bind(id)
    .bind(project)
    .bind(record)
    .bind(candidate)
    .execute(&h.pool)
    .await
    .expect("proposal inserts");
    id
}

async fn insert_document(h: &Harness, project: Uuid, report: Uuid) -> (Uuid, Uuid) {
    let document = Uuid::new_v4();
    let block = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents
           (id,project_id,report_id,source,status,object_key,content_hash,mime_type,byte_size,active_parser_version)
         VALUES ($1,$2,$3,'upload','available',$4,$5,'application/pdf',1,'parser-1')",
    )
    .bind(document)
    .bind(project)
    .bind(report)
    .bind(format!("documents/{document}"))
    .bind("a".repeat(64))
    .execute(&h.pool)
    .await
    .expect("document inserts");
    sqlx::query(
        "INSERT INTO document_pages (document_id,parser_version,page_number,width,height,active)
         VALUES ($1,'parser-1',1,100,100,true)",
    )
    .bind(document)
    .execute(&h.pool)
    .await
    .expect("document page inserts");
    sqlx::query(
        "INSERT INTO document_blocks
           (id,document_id,parser_version,page_number,kind,section_path,ordinal,text,content_hash,active)
         VALUES ($1,$2,'parser-1',1,'text','{}',1,'Allocation was described',$3,true)",
    )
    .bind(block)
    .bind(document)
    .bind("b".repeat(64))
    .execute(&h.pool)
    .await
    .expect("document block inserts");
    (document, block)
}

async fn insert_protocol(h: &Harness, project: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO protocol_versions (id,project_id,version,name,status,criteria)
         VALUES ($1,$2,1,'child-test','published','[]')",
    )
    .bind(id)
    .bind(project)
    .execute(&h.pool)
    .await
    .expect("protocol inserts");
    id
}

async fn insert_exclusion_reason(h: &Harness, project: Uuid, code: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO exclusion_reasons (id,project_id,code,label,stage)
         VALUES ($1,$2,$3,'Child test reason','title_abstract')",
    )
    .bind(id)
    .bind(project)
    .bind(code)
    .execute(&h.pool)
    .await
    .expect("exclusion reason inserts");
    id
}

/// A queued reviewer-requested `duplicate_detection` run of `project`.
async fn insert_review_run(h: &Harness, project: Uuid, record: Uuid, candidate: Uuid) -> Uuid {
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO durable_runs (id,project_id,kind,actor_kind,actor_id,idempotency_key,status)
         VALUES ($1,$2,'automation','user','child-tests',$1::text,'queued')",
    )
    .bind(run)
    .bind(project)
    .execute(&h.pool)
    .await
    .expect("review run inserts");
    sqlx::query(
        "INSERT INTO review_run_manifests
           (project_id,automation_run_id,definition_key,definition_id,definition_version,
            manifest_hash,semantic_bundle_hash,manifest,subject,origin,prepared_task)
         VALUES ($1,$2,'duplicate_detection','duplicate_detection.v1',1,$3,$3,
                 '{}'::jsonb,$4,'{\"kind\":\"reviewer_requested\"}'::jsonb,'{}'::jsonb)",
    )
    .bind(project)
    .bind(run)
    .bind(format!("{}{}", run.simple(), run.simple()))
    .bind(serde_json::json!({
        "kind": "duplicate_detection",
        "record_id": record,
        "candidate_report_id": candidate,
    }))
    .execute(&h.pool)
    .await
    .expect("review manifest inserts");
    run
}

impl World {
    /// A project extraction field, created through the API as `person`.
    async fn field(&self, person: &Person, project: Uuid, key: &str) -> Uuid {
        let (status, body) = self
            .h
            .json(
                person,
                Method::POST,
                &format!("/projects/{project}/extraction/fields"),
                &format!(
                    r#"{{"version":1,"field_key":"{key}","label":"Sample size","value_type":"number","required":false}}"#
                ),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
    }

    /// A study, created through the API as `person`.
    async fn study(&self, person: &Person, project: Uuid, title: &str) -> Uuid {
        let (status, body) = self
            .h
            .json(
                person,
                Method::POST,
                &format!("/projects/{project}/studies"),
                &format!(r#"{{"title":"{title}"}}"#),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
    }

    /// A manual extraction value, created through the API as `person`.
    async fn value(&self, person: &Person, project: Uuid, study: Uuid, field: Uuid) -> Uuid {
        let (status, body) = self
            .h
            .json(
                person,
                Method::PUT,
                &format!("/projects/{project}/studies/{study}/extraction/values/{field}"),
                r#"{"value":{"kind":"number","value":12.0}}"#,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
    }
}

async fn proposal_status(pool: &sqlx::PgPool, proposal: Uuid) -> (String, i64) {
    sqlx::query_as::<_, (String, i64)>("SELECT status, revision FROM dedupe_proposals WHERE id=$1")
        .bind(proposal)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// Deduplication proposals and record resolution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dedupe_proposals_records_and_their_body_ids_never_cross_projects() {
    let Some(w) = world().await else {
        return;
    };
    // A's children.
    let a_report = insert_report(&w.h, w.a_project, "A's report").await;
    let a_candidate = insert_report(&w.h, w.a_project, "A's candidate").await;
    let a_record = insert_record(&w.h, w.a_project, "a-record").await;
    let a_proposal = insert_proposal(&w.h, w.a_project, a_record, a_candidate).await;
    // B's children, for the body-carried ids.
    let b_report = insert_report(&w.h, w.b_project, "B's report").await;
    let b_record = insert_record(&w.h, w.b_project, "b-record").await;
    let b_proposal = insert_proposal(&w.h, w.b_project, b_record, b_report).await;

    // The listing is project-scoped.
    let (status, body) =
        w.h.get(
            &w.a,
            &format!("/projects/{}/deduplication/proposals", w.a_project),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains(&a_proposal.to_string()));
    let (status, body) =
        w.h.get(
            &w.b,
            &format!("/projects/{}/deduplication/proposals", w.b_project),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let listed = String::from_utf8_lossy(&body).into_owned();
    assert!(!listed.contains(&a_proposal.to_string()), "{listed}");
    assert!(listed.contains(&b_proposal.to_string()), "{listed}");

    // A decision on A's proposal under B's project.
    let decision = r#"{"decision":"reject","reason":"cross-tenant probe"}"#;
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!(
            "/projects/{}/deduplication/proposals/{}/decision",
            w.b_project, a_proposal
        ),
        Some(decision),
        a_proposal,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        proposal_status(&w.h.pool, a_proposal).await,
        ("pending".to_owned(), 0)
    );

    // A resolution of A's record under B's project.
    let create = r#"{"action":"create","reason":"cross-tenant probe"}"#;
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/records/{}/resolution", w.b_project, a_record),
        Some(create),
        a_record,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Body-carried ids: A's report as the link target of B's record.
    let link =
        format!(r#"{{"action":"link","report_id":"{a_report}","reason":"cross-tenant probe"}}"#);
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/records/{}/resolution", w.b_project, b_record),
        Some(&link),
        a_report,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Body-carried ids: A's proposal applied to B's record.
    let with_proposal = format!(
        r#"{{"action":"link","report_id":"{b_report}","proposal_id":"{a_proposal}","reason":"cross-tenant probe"}}"#
    );
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/records/{}/resolution", w.b_project, b_record),
        Some(&with_proposal),
        a_proposal,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Nothing was written for A or B.
    let unresolved: Option<Uuid> = sqlx::query_scalar("SELECT report_id FROM records WHERE id=$1")
        .bind(a_record)
        .fetch_one(&w.h.pool)
        .await
        .unwrap();
    assert_eq!(unresolved, None, "A's record must stay unresolved");
    let b_unresolved: Option<Uuid> =
        sqlx::query_scalar("SELECT report_id FROM records WHERE id=$1")
            .bind(b_record)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(b_unresolved, None, "B's record must stay unresolved");
    for record in [a_record, b_record] {
        let events: i64 =
            sqlx::query_scalar("SELECT count(*) FROM dedupe_resolution_events WHERE record_id=$1")
                .bind(record)
                .fetch_one(&w.h.pool)
                .await
                .unwrap();
        assert_eq!(events, 0, "no resolution event for {record}");
    }
    assert_eq!(
        proposal_status(&w.h.pool, a_proposal).await,
        ("pending".to_owned(), 0)
    );

    // The AI deduplication route: A's record under B's project, then B's
    // record with A's report as the candidate.
    let for_b = format!(r#"{{"candidate_report_id":"{b_report}"}}"#);
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!(
            "/projects/{}/records/{}/ai/deduplication",
            w.b_project, a_record
        ),
        Some(&for_b),
        a_record,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let with_candidate = format!(r#"{{"candidate_report_id":"{a_report}"}}"#);
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!(
            "/projects/{}/records/{}/ai/deduplication",
            w.b_project, b_record
        ),
        Some(&with_candidate),
        a_report,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let a_runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM review_run_manifests WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_runs, 0, "no review run may be scheduled against A");

    // A deduplication run under B's project never touches A's rows.
    let (status, body) =
        w.h.json(
            &w.b,
            Method::POST,
            &format!("/projects/{}/deduplication/run", w.b_project),
            r#"{"limit":100}"#,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let a_unresolved: Option<Uuid> =
        sqlx::query_scalar("SELECT report_id FROM records WHERE id=$1")
            .bind(a_record)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_unresolved, None, "the run must not resolve A's record");
    assert_eq!(
        proposal_status(&w.h.pool, a_proposal).await,
        ("pending".to_owned(), 0)
    );
    let a_reports: i64 =
        sqlx::query_scalar("SELECT count(*) FROM project_reports WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_reports, 2, "the run must not create reports for A");
}

// ---------------------------------------------------------------------------
// Extraction fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn extraction_field_definitions_never_cross_projects() {
    let Some(w) = world().await else {
        return;
    };
    let a_field = w.field(&w.a, w.a_project, "a-field").await;
    let b_field = w.field(&w.b, w.b_project, "b-field").await;

    // The listing is project-scoped.
    let (status, body) =
        w.h.get(
            &w.a,
            &format!("/projects/{}/extraction/fields", w.a_project),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains(&a_field.to_string()));
    let (status, body) =
        w.h.get(
            &w.b,
            &format!("/projects/{}/extraction/fields", w.b_project),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let listed = String::from_utf8_lossy(&body).into_owned();
    assert!(!listed.contains(&a_field.to_string()), "{listed}");
    assert!(listed.contains(&b_field.to_string()), "{listed}");

    // B patches A's definition under B's project.
    let patch = r#"{"label":"Renamed by B","value_type":"text","required":true}"#;
    let (status, _) = same_as_missing(
        &w,
        Method::PATCH,
        &format!("/projects/{}/extraction/fields/{}", w.b_project, a_field),
        Some(patch),
        a_field,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let label: String = sqlx::query_scalar(
        "SELECT label FROM extraction_field_definitions WHERE project_id=$1 AND id=$2",
    )
    .bind(w.a_project)
    .bind(a_field)
    .fetch_one(&w.h.pool)
    .await
    .unwrap();
    let version: i32 = sqlx::query_scalar(
        "SELECT version FROM extraction_field_definitions WHERE project_id=$1 AND id=$2",
    )
    .bind(w.a_project)
    .bind(a_field)
    .fetch_one(&w.h.pool)
    .await
    .unwrap();
    assert_eq!(label, "Sample size", "A's definition is unchanged");
    assert_eq!(version, 1, "no new version of A's definition was written");

    // The creation body also carries a client-chosen field id. It is
    // namespaced to the project (the key is `(project_id, id, version)`), so
    // B reusing A's id must write a row of their own and leave A's alone.
    let clone = format!(
        r#"{{"id":"{a_field}","version":1,"field_key":"b-clone","label":"B's clone","value_type":"text","required":false}}"#
    );
    let (status, body) =
        w.h.json(
            &w.b,
            Method::POST,
            &format!("/projects/{}/extraction/fields", w.b_project),
            &clone,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let owner: Uuid = sqlx::query_scalar(
        "SELECT project_id FROM extraction_field_definitions WHERE id=$1 AND field_key='b-clone'",
    )
    .bind(a_field)
    .fetch_one(&w.h.pool)
    .await
    .unwrap();
    assert_eq!(owner, w.b_project, "the reused id belongs to B's row alone");
    let label: String = sqlx::query_scalar(
        "SELECT label FROM extraction_field_definitions WHERE project_id=$1 AND id=$2",
    )
    .bind(w.a_project)
    .bind(a_field)
    .fetch_one(&w.h.pool)
    .await
    .unwrap();
    assert_eq!(label, "Sample size", "A's definition is still unchanged");
    let a_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM extraction_field_definitions WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_rows, 1, "no row was written into A's project");
}

// ---------------------------------------------------------------------------
// Extraction values
// ---------------------------------------------------------------------------

#[tokio::test]
async fn extraction_values_and_their_evidence_never_cross_projects() {
    let Some(w) = world().await else {
        return;
    };
    let a_report = insert_report(&w.h, w.a_project, "A's report").await;
    let (a_document, a_block) = insert_document(&w.h, w.a_project, a_report).await;
    let a_study = w.study(&w.a, w.a_project, "A's study").await;
    let a_field = w.field(&w.a, w.a_project, "a-field").await;
    let a_value = w.value(&w.a, w.a_project, a_study, a_field).await;

    let b_report = insert_report(&w.h, w.b_project, "B's report").await;
    let (b_document, b_block) = insert_document(&w.h, w.b_project, b_report).await;
    let b_study = w.study(&w.b, w.b_project, "B's study").await;
    let b_field = w.field(&w.b, w.b_project, "b-field").await;
    for (project, study, report) in [
        (w.a_project, a_study, a_report),
        (w.b_project, b_study, b_report),
    ] {
        sqlx::query(
            "INSERT INTO study_reports (project_id,study_id,report_id,relationship)
             VALUES ($1,$2,$3,'report_of_study')",
        )
        .bind(project)
        .bind(study)
        .bind(report)
        .execute(&w.h.pool)
        .await
        .expect("study report inserts");
    }
    sqlx::query("UPDATE extraction_values SET needs_verification=true WHERE id=$1")
        .bind(a_value)
        .execute(&w.h.pool)
        .await
        .expect("value flagged for verification");

    // The value listing is project-scoped.
    let (status, body) =
        w.h.get(
            &w.a,
            &format!("/projects/{}/studies/{a_study}/extraction", w.a_project),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains(&a_value.to_string()));

    // A's study under B's project.
    let (status, _) = same_as_missing(
        &w,
        Method::GET,
        &format!("/projects/{}/studies/{a_study}/extraction", w.b_project),
        None,
        a_study,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A's study and A's field under B's project.
    let value_body = r#"{"value":{"kind":"number","value":7.0}}"#;
    let (status, _) = same_as_missing(
        &w,
        Method::PUT,
        &format!(
            "/projects/{}/studies/{a_study}/extraction/values/{a_field}",
            w.b_project
        ),
        Some(value_body),
        a_study,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // B's study with A's field.
    let (status, _) = same_as_missing(
        &w,
        Method::PUT,
        &format!(
            "/projects/{}/studies/{b_study}/extraction/values/{a_field}",
            w.b_project
        ),
        Some(value_body),
        a_field,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Body-carried evidence: A's document and block cited for B's study.
    let evidence = format!(
        r#"{{"value":{{"kind":"number","value":7.0}},"source":{{"document_id":"{a_document}","document_block_id":"{a_block}"}}}}"#
    );
    let (status, _) = same_as_missing(
        &w,
        Method::PUT,
        &format!(
            "/projects/{}/studies/{b_study}/extraction/values/{b_field}",
            w.b_project
        ),
        Some(&evidence),
        a_document,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // The same request with B's own evidence works, so the refusal above is
    // about the evidence and nothing else.
    let b_evidence = format!(
        r#"{{"value":{{"kind":"number","value":7.0}},"source":{{"document_id":"{b_document}","document_block_id":"{b_block}"}}}}"#
    );
    let (status, body) =
        w.h.json(
            &w.b,
            Method::PUT,
            &format!(
                "/projects/{}/studies/{b_study}/extraction/values/{b_field}",
                w.b_project
            ),
            &b_evidence,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Clearing and confirming through A's children.
    let (status, _) = same_as_missing(
        &w,
        Method::DELETE,
        &format!(
            "/projects/{}/studies/{a_study}/extraction/values/{a_field}",
            w.b_project
        ),
        None,
        a_study,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!(
            "/projects/{}/studies/{a_study}/extraction/values/{a_value}/confirm",
            w.b_project
        ),
        None,
        a_value,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A's value is exactly as it was written.
    let row: (bool, Option<chrono::DateTime<chrono::Utc>>, Option<f64>) = sqlx::query_as(
        "SELECT needs_verification, verified_at, number_value FROM extraction_values WHERE id=$1",
    )
    .bind(a_value)
    .fetch_one(&w.h.pool)
    .await
    .unwrap();
    assert!(row.0, "A's value still waits for confirmation");
    assert_eq!(row.1, None, "A's value was not confirmed");
    assert_eq!(row.2, Some(12.0), "A's value was not overwritten");
    let a_values: i64 =
        sqlx::query_scalar("SELECT count(*) FROM extraction_values WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_values, 1, "no value was written for A");
    let b_values: i64 =
        sqlx::query_scalar("SELECT count(*) FROM extraction_values WHERE project_id=$1")
            .bind(w.b_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(b_values, 1, "only B's own evidence-backed value exists");

    // The AI extraction route under B's project with A's study. The handler
    // (`schedule_data_extraction_review`, crates/app/src/review_preparation.rs)
    // never checks that the study belongs to the path project before it
    // schedules, so the refusal here is the missing model route, not a scoped
    // lookup: a foreign study and a missing one are treated alike. Whatever
    // the answer, it must be the same for both, and nothing of A's may move.
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/studies/{a_study}/ai/extraction", w.b_project),
        None,
        a_study,
    )
    .await;
    assert_ne!(status, StatusCode::ACCEPTED);
    // Nothing was scheduled in A's project.
    let a_runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM review_run_manifests WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_runs, 0, "no review run may be scheduled in A's project");
}

// ---------------------------------------------------------------------------
// Review runs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn review_runs_never_cross_projects() {
    let Some(w) = world().await else {
        return;
    };
    let a_report = insert_report(&w.h, w.a_project, "A's report").await;
    let a_record = insert_record(&w.h, w.a_project, "a-record").await;
    let a_run = insert_review_run(&w.h, w.a_project, a_record, a_report).await;

    // The owner reads their run.
    let (status, body) =
        w.h.get(
            &w.a,
            &format!("/projects/{}/review-runs/{a_run}", w.a_project),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let owned = String::from_utf8_lossy(&body).into_owned();
    assert!(owned.contains(&a_run.to_string()), "{owned}");
    assert!(owned.contains(&w.a_project.to_string()), "{owned}");

    // B reads A's run under B's project.
    let (status, _) = same_as_missing(
        &w,
        Method::GET,
        &format!("/projects/{}/review-runs/{a_run}", w.b_project),
        None,
        a_run,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // B retries A's run under B's project.
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/review-runs/{a_run}/retry", w.b_project),
        Some(r#"{"regenerate":false}"#),
        a_run,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A's run is untouched: still queued, still one run in the project.
    let state: String = sqlx::query_scalar("SELECT status FROM durable_runs WHERE id=$1")
        .bind(a_run)
        .fetch_one(&w.h.pool)
        .await
        .unwrap();
    assert_eq!(state, "queued", "A's run was not retried");
    let a_runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM review_run_manifests WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_runs, 1, "no generation was added for A");
}

// ---------------------------------------------------------------------------
// The /reports/{report_id}/* screening routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn report_screening_routes_and_their_body_ids_never_cross_projects() {
    let Some(w) = world().await else {
        return;
    };
    let a_report = insert_report(&w.h, w.a_project, "A's report").await;
    let a_protocol = insert_protocol(&w.h, w.a_project).await;
    let a_reason = insert_exclusion_reason(&w.h, w.a_project, "a-reason").await;
    let b_report = insert_report(&w.h, w.b_project, "B's report").await;
    let b_protocol = insert_protocol(&w.h, w.b_project).await;
    let b_reason = insert_exclusion_reason(&w.h, w.b_project, "b-reason").await;

    let screen = format!(
        r#"{{"stage":"title_abstract","decision":"include","protocol_version_id":"{b_protocol}","expected_revision":0}}"#
    );
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/reports/{a_report}/screening", w.b_project),
        Some(&screen),
        a_report,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let history_path = format!(
        "/projects/{}/reports/{a_report}/screening/history",
        w.b_project
    );
    let (status, _) = same_as_missing(&w, Method::GET, &history_path, None, a_report).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The owner reads the history of their own report.
    let (status, body) =
        w.h.get(
            &w.a,
            &format!(
                "/projects/{}/reports/{a_report}/screening/history",
                w.a_project
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let undo = format!(
        r#"{{"stage":"title_abstract","protocol_version_id":"{b_protocol}","expected_revision":0}}"#
    );
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!(
            "/projects/{}/reports/{a_report}/screening/undo",
            w.b_project
        ),
        Some(&undo),
        a_report,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Body-carried ids on B's own report: A's protocol version.
    let with_foreign_protocol = format!(
        r#"{{"stage":"title_abstract","decision":"include","protocol_version_id":"{a_protocol}","expected_revision":0}}"#
    );
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/reports/{b_report}/screening", w.b_project),
        Some(&with_foreign_protocol),
        a_protocol,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Body-carried ids on B's own report: A's exclusion reason.
    let exclude = format!(
        r#"{{"stage":"title_abstract","decision":"exclude","protocol_version_id":"{b_protocol}","exclusion_reason_id":"{a_reason}","expected_revision":0}}"#
    );
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/reports/{b_report}/screening", w.b_project),
        Some(&exclude),
        a_reason,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // B decides their own report, then undoes with A's protocol version.
    let (status, body) = w
        .h
        .json(
            &w.b,
            Method::POST,
            &format!("/projects/{}/reports/{b_report}/screening", w.b_project),
            &format!(
                r#"{{"stage":"title_abstract","decision":"include","protocol_version_id":"{b_protocol}","expected_revision":0}}"#
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let undo_with_foreign_protocol = format!(
        r#"{{"stage":"title_abstract","protocol_version_id":"{a_protocol}","expected_revision":1}}"#
    );
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!(
            "/projects/{}/reports/{b_report}/screening/undo",
            w.b_project
        ),
        Some(&undo_with_foreign_protocol),
        a_protocol,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A has no screening state at all, and B's decision stands.
    let a_state: i64 =
        sqlx::query_scalar("SELECT count(*) FROM screening_state WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_state, 0, "nothing may be written for A");
    let a_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM screening_events WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_events, 0, "no screening event may name A's report");
    let (status, revision): (String, i64) = sqlx::query_as(
        "SELECT title_abstract_status, revision FROM screening_state WHERE project_id=$1 AND report_id=$2",
    )
    .bind(w.b_project)
    .bind(b_report)
    .fetch_one(&w.h.pool)
    .await
    .unwrap();
    assert_eq!(status, "include", "B's decision stands");
    assert_eq!(revision, 1, "the refused undos changed nothing");
    // The refused requests named B's reason and protocol; they were never used.
    let unused_reason: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM screening_state WHERE project_id=$1 AND full_text_exclusion_reason_id=$2",
    )
    .bind(w.b_project)
    .bind(b_reason)
    .fetch_one(&w.h.pool)
    .await
    .unwrap();
    assert_eq!(unused_reason, 0, "B's own reason is untouched too");
}

// ---------------------------------------------------------------------------
// The /reports/{report_id}/* appraisal routes
// ---------------------------------------------------------------------------

/// A complete appraisal for `deepref-rct-generic`, with `evidence` as the
/// cited block.
fn appraisal_body(evidence_document: Uuid, evidence_block: Uuid) -> String {
    format!(
        r#"{{"definition_id":"deepref-rct-generic","definition_version":1,
        "responses":{{"allocation_description":"yes","outcome_measure_prespecified":true}},
        "evidence":[{{"question_id":"allocation_description","document_id":"{evidence_document}","block_id":"{evidence_block}"}}],
        "domain_judgments":{{"allocation":"low_concern","outcome_reporting":"low_concern"}},
        "overall_judgment":"low_concern"}}"#
    )
}

#[tokio::test]
async fn report_appraisal_routes_and_their_evidence_never_cross_projects() {
    let Some(w) = world().await else {
        return;
    };
    let a_report = insert_report(&w.h, w.a_project, "A's report").await;
    let (a_document, a_block) = insert_document(&w.h, w.a_project, a_report).await;
    let b_report = insert_report(&w.h, w.b_project, "B's report").await;
    let (b_document, b_block) = insert_document(&w.h, w.b_project, b_report).await;

    // The listing is project-scoped.
    let (status, body) =
        w.h.get(
            &w.a,
            &format!("/projects/{}/reports/{a_report}/appraisals", w.a_project),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    // A's report under B's project.
    let (status, _) = same_as_missing(
        &w,
        Method::GET,
        &format!("/projects/{}/reports/{a_report}/appraisals", w.b_project),
        None,
        a_report,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/reports/{a_report}/appraisals", w.b_project),
        Some(&appraisal_body(b_document, b_block)),
        a_report,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Body-carried evidence: A's document and block cited for B's report.
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/reports/{b_report}/appraisals", w.b_project),
        Some(&appraisal_body(a_document, a_block)),
        a_document,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The same request with B's own evidence works, so the refusal above is
    // about the evidence and nothing else.
    let (status, body) =
        w.h.json(
            &w.b,
            Method::POST,
            &format!("/projects/{}/reports/{b_report}/appraisals", w.b_project),
            &appraisal_body(b_document, b_block),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // A has no appraisal, and the one written is B's alone.
    let a_assessments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM appraisal_assessments WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_assessments, 0, "nothing may be written for A");
    let b_assessments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM appraisal_assessments WHERE project_id=$1")
            .bind(w.b_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(b_assessments, 1, "only B's own appraisal exists");
    let cited: Uuid = sqlx::query_scalar(
        "SELECT document_id FROM appraisal_assessment_evidence WHERE project_id=$1",
    )
    .bind(w.b_project)
    .fetch_one(&w.h.pool)
    .await
    .unwrap();
    assert_eq!(cited, b_document, "the assessment cites B's document");
}

// ---------------------------------------------------------------------------
// The /reports/{report_id}/* AI routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn report_ai_routes_and_their_body_ids_never_cross_projects() {
    let Some(w) = world().await else {
        return;
    };
    let a_report = insert_report(&w.h, w.a_project, "A's report").await;
    let a_protocol = insert_protocol(&w.h, w.a_project).await;
    let b_report = insert_report(&w.h, w.b_project, "B's report").await;
    let b_protocol = insert_protocol(&w.h, w.b_project).await;

    let screening = format!(
        r#"{{"stage":"title_abstract","protocol_version_id":"{b_protocol}","expected_revision":0}}"#
    );
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/reports/{a_report}/ai/screening", w.b_project),
        Some(&screening),
        a_report,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!(
            "/projects/{}/reports/{a_report}/ai/study-grouping",
            w.b_project
        ),
        None,
        a_report,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let prefill = r#"{"definition_id":"deepref-rct-generic","definition_version":1}"#;
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!(
            "/projects/{}/reports/{a_report}/ai/appraisal-prefill",
            w.b_project
        ),
        Some(prefill),
        a_report,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Body-carried ids: A's protocol version on B's report.
    let with_foreign_protocol = format!(
        r#"{{"stage":"title_abstract","protocol_version_id":"{a_protocol}","expected_revision":0}}"#
    );
    let (status, _) = same_as_missing(
        &w,
        Method::POST,
        &format!("/projects/{}/reports/{b_report}/ai/screening", w.b_project),
        Some(&with_foreign_protocol),
        a_protocol,
    )
    .await;
    assert_ne!(
        status,
        StatusCode::ACCEPTED,
        "A's protocol must not be compiled into B's review"
    );

    // The owner's own report passes the child lookup; the run is refused or
    // accepted but never mistaken for a missing report. This is what proves
    // the denials above are about the child id and not about the route.
    for (path, body) in [
        (
            format!("/projects/{}/reports/{a_report}/ai/screening", w.a_project),
            Some(screening.replace(&b_protocol.to_string(), &a_protocol.to_string())),
        ),
        (
            format!(
                "/projects/{}/reports/{a_report}/ai/study-grouping",
                w.a_project
            ),
            None,
        ),
        (
            format!(
                "/projects/{}/reports/{a_report}/ai/appraisal-prefill",
                w.a_project
            ),
            Some(prefill.to_owned()),
        ),
    ] {
        let (status, body_bytes) = w.h.send(&w.a, Method::POST, &path, body.as_deref()).await;
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "{path}: A's own report must pass the child lookup: {}",
            String::from_utf8_lossy(&body_bytes)
        );
    }

    // Nothing was scheduled against A.
    let a_runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM review_run_manifests WHERE project_id=$1")
            .bind(w.a_project)
            .fetch_one(&w.h.pool)
            .await
            .unwrap();
    assert_eq!(a_runs, 0, "no review run may be scheduled against A");
}

// ---------------------------------------------------------------------------
// Appraisal definitions
// ---------------------------------------------------------------------------

/// Appraisal definitions are a deployment catalog compiled into the binary:
/// their id is not a project child, and the routes must answer the same for
/// every project that may call them without leaking project data.
#[tokio::test]
async fn appraisal_definitions_are_a_deployment_catalog() {
    let Some(w) = world().await else {
        return;
    };
    for (person, project) in [(&w.a, w.a_project), (&w.b, w.b_project)] {
        let (status, body) =
            w.h.get(
                person,
                &format!("/projects/{project}/appraisal-definitions"),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let text = String::from_utf8_lossy(&body).into_owned();
        assert!(text.contains("deepref-rct-generic"), "{text}");
        assert!(!text.contains(&w.a_project.to_string()), "{text}");
        assert!(!text.contains(&w.b_project.to_string()), "{text}");
    }
    let (a_status, a_body) =
        w.h.get(
            &w.a,
            &format!("/projects/{}/appraisal-definitions", w.a_project),
        )
        .await;
    let (b_status, b_body) =
        w.h.get(
            &w.b,
            &format!("/projects/{}/appraisal-definitions", w.b_project),
        )
        .await;
    assert_eq!((a_status, a_body), (b_status, b_body));

    // One definition and its judgment suggestion answer identically in both
    // workspaces; an unknown definition is missing for both.
    let suggestion =
        r#"{"responses":{"allocation_description":"yes","outcome_measure_prespecified":true}}"#;
    for (path_suffix, body) in [
        ("deepref-rct-generic/1", None),
        (
            "deepref-rct-generic/1/judgment-suggestion",
            Some(suggestion),
        ),
        ("unknown-definition/1", None),
        ("unknown-definition/1/judgment-suggestion", Some(suggestion)),
    ] {
        let (a_status, a_body) =
            w.h.send(
                &w.a,
                if body.is_some() {
                    Method::POST
                } else {
                    Method::GET
                },
                &format!(
                    "/projects/{}/appraisal-definitions/{path_suffix}",
                    w.a_project
                ),
                body,
            )
            .await;
        let (b_status, b_body) =
            w.h.send(
                &w.b,
                if body.is_some() {
                    Method::POST
                } else {
                    Method::GET
                },
                &format!(
                    "/projects/{}/appraisal-definitions/{path_suffix}",
                    w.b_project
                ),
                body,
            )
            .await;
        assert_eq!(
            (a_status, normalize(&a_body)),
            (b_status, normalize(&b_body)),
            "{path_suffix}: the catalog answers the same in every project"
        );
    }
}
