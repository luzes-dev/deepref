#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! Cross-tenant ID guessing, end to end (handoff 03b).
//!
//! The table in `authz::policy` is the coverage matrix, and this file iterates
//! it: every `Require` row is called as a principal of workspace B with an
//! identifier of workspace A, and the response must equal the response for a
//! project that does not exist. A route added to the table is covered by the
//! next run with no edit here.
use axum::http::{Method, StatusCode};
use deepref_domain::{WorkspaceRole, authorization::Permission};
use deepref_http_api::authz::policy::{ROUTES, ResourceFrom, Rule, Verb};
use uuid::Uuid;

mod support;
use support::authz::{Harness, Person};

/// A concrete path for a route template: `project_id` is given, parameters that
/// are not UUIDs get a plausible token, everything else a random UUID.
fn concrete(template: &str, project: Uuid) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}').unwrap() + start;
        let name = &rest[start + 1..end];
        match name {
            "project_id" => out.push_str(&project.to_string()),
            "version" => out.push('1'),
            "recipe" | "export_kind" | "definition_id" => out.push('x'),
            _ => out.push_str(&Uuid::new_v4().to_string()),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn method(verb: Verb) -> Method {
    Method::from_bytes(verb.as_str().as_bytes()).unwrap()
}

fn project_rows() -> Vec<&'static deepref_http_api::authz::policy::RouteRule> {
    ROUTES
        .iter()
        .filter(|row| {
            matches!(
                row.rule,
                Rule::Require(_, ResourceFrom::PathParam("project_id"))
            )
        })
        .collect()
}

struct Two {
    h: Harness,
    a: Person,
    b: Person,
    a_project: Uuid,
    b_project: Uuid,
}

async fn two_workspaces() -> Option<Two> {
    let h = Harness::new().await?;
    let a = h.sign_in("user_A", "a@example.org").await;
    let b = h.sign_in("user_B", "b@example.org").await;
    let a_project = h.create_project(&a, "A's project").await;
    let b_project = h.create_project(&b, "B's project").await;
    Some(Two {
        h,
        a,
        b,
        a_project,
        b_project,
    })
}

#[tokio::test]
async fn foreign_project_is_indistinguishable_from_a_missing_one() {
    let Some(t) = two_workspaces().await else {
        return;
    };
    let rows = project_rows();
    assert!(
        rows.len() > 100,
        "the table drives this test: {} rows",
        rows.len()
    );
    let mut failures = Vec::new();
    for row in &rows {
        let foreign = concrete(row.path, t.a_project);
        // The same child identifiers on both sides, so only the project differs.
        let missing = foreign.replace(&t.a_project.to_string(), &Uuid::new_v4().to_string());
        let (foreign_status, foreign_body) =
            t.h.send(&t.b, method(row.method), &foreign, None).await;
        let (missing_status, missing_body) =
            t.h.send(&t.b, method(row.method), &missing, None).await;
        let label = format!("{} {}", row.method.as_str(), row.path);
        if foreign_status != StatusCode::NOT_FOUND {
            failures.push(format!(
                "{label}: foreign project answered {foreign_status}"
            ));
        }
        if foreign_status != missing_status || normalize(&foreign_body) != normalize(&missing_body)
        {
            failures.push(format!(
                "{label}: foreign {foreign_status} {} != missing {missing_status} {}",
                String::from_utf8_lossy(&foreign_body),
                String::from_utf8_lossy(&missing_body)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The body with project identifiers replaced, in case a message ever echoes
/// the id it was asked about.
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

#[tokio::test]
async fn the_denial_is_byte_identical_to_the_existing_missing_project_answer() {
    let Some(t) = two_workspaces().await else {
        return;
    };
    let (status, body) = t.h.get(&t.b, &format!("/projects/{}", t.a_project)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["code"], "NOT_FOUND");
    assert_eq!(body["message"], "resource not found");
    // The same answer the handler gave for a project that does not exist
    // before the PEP existed: `RowNotFound` rendered by `ApiError`.
    let rendered = {
        use axum::response::IntoResponse;
        let response =
            deepref_http_api::error::ApiError::Database(sqlx::Error::RowNotFound).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
    };
    assert_eq!(body["code"], rendered["code"]);
    assert_eq!(body["message"], rendered["message"]);
}

#[tokio::test]
async fn an_owner_still_reaches_their_own_project() {
    let Some(t) = two_workspaces().await else {
        return;
    };
    let p = t.a_project;
    for path in [
        format!("/projects/{p}"),
        format!("/projects/{p}/reports"),
        format!("/projects/{p}/screening"),
        format!("/projects/{p}/studies"),
        format!("/projects/{p}/review/protocol"),
        format!("/projects/{p}/acquisitions"),
        format!("/projects/{p}/automations/workflows"),
        format!("/projects/{p}/assistant/conversations"),
        format!("/projects/{p}/ai/budget"),
    ] {
        let (status, body) = t.h.get(&t.a, &path).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{path}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let (status, _) =
        t.h.json(
            &t.a,
            Method::PATCH,
            &format!("/projects/{p}"),
            r#"{"name":"Renamed"}"#,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // B keeps their own.
    let (status, _) = t.h.get(&t.b, &format!("/projects/{}", t.b_project)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn membership_removal_takes_effect_on_the_next_request() {
    let Some(t) = two_workspaces().await else {
        return;
    };
    t.h.add_member(&t.a, &t.b, WorkspaceRole::Member).await;
    let path = format!("/projects/{}", t.a_project);
    let (status, _) = t.h.get(&t.b, &path).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a member sees the workspace's project"
    );
    let (status, _) =
        t.h.json(&t.b, Method::PATCH, &path, r#"{"name":"Edited by member"}"#)
            .await;
    assert_eq!(status, StatusCode::OK, "a member may edit");

    deepref_app::workspaces::remove_membership(
        &t.h.pool,
        t.a.user,
        t.a.workspace,
        t.b.user,
        deepref_app::identity::AuditRequest::default(),
    )
    .await
    .unwrap();
    let (status, body) = t.h.get(&t.b, &path).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "removal is visible immediately"
    );
    let (_, missing) =
        t.h.get(&t.b, &format!("/projects/{}", Uuid::new_v4()))
            .await;
    assert_eq!(normalize(&body), normalize(&missing));
    // The user keeps their own workspace.
    let (status, _) = t.h.get(&t.b, &format!("/projects/{}", t.b_project)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_role_downgrade_takes_effect_on_the_next_request() {
    let Some(t) = two_workspaces().await else {
        return;
    };
    t.h.add_member(&t.a, &t.b, WorkspaceRole::Admin).await;
    let path = format!("/projects/{}/automations/workflows", t.a_project);
    let (status, _) =
        t.h.json(&t.b, Method::POST, &path, r#"{"name":"flow"}"#)
            .await;
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "an admin holds automation:manage"
    );
    deepref_app::workspaces::change_membership_role(
        &t.h.pool,
        t.a.user,
        t.a.workspace,
        t.b.user,
        WorkspaceRole::Member,
        deepref_app::identity::AuditRequest::default(),
    )
    .await
    .unwrap();
    let (status, _) =
        t.h.json(&t.b, Method::POST, &path, r#"{"name":"flow"}"#)
            .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a member lacks automation:manage"
    );
    // Still a member: viewing and editing remain.
    let (status, _) = t.h.get(&t.b, &format!("/projects/{}", t.a_project)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) =
        t.h.send(
            &t.b,
            Method::DELETE,
            &format!("/projects/{}", t.a_project),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a member lacks project:delete"
    );
    let (status, _) = t.h.get(&t.a, &format!("/projects/{}", t.a_project)).await;
    assert_eq!(status, StatusCode::OK, "the denied delete changed nothing");
}

#[tokio::test]
async fn a_project_grant_reaches_one_project_and_only_what_its_role_carries() {
    let Some(t) = two_workspaces().await else {
        return;
    };
    let second = t.h.create_project(&t.a, "A's second project").await;
    deepref_app::workspaces::grant_project_access(
        &t.h.pool,
        t.a.user,
        deepref_domain::ProjectId::new(t.a_project),
        t.b.user,
        deepref_domain::ProjectRole::Viewer,
        deepref_app::identity::AuditRequest::default(),
    )
    .await
    .unwrap();
    let granted = format!("/projects/{}", t.a_project);
    let (status, _) = t.h.get(&t.b, &granted).await;
    assert_eq!(status, StatusCode::OK, "the viewer grant reads the project");
    let (status, _) =
        t.h.json(&t.b, Method::PATCH, &granted, r#"{"name":"nope"}"#)
            .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a viewer cannot edit");
    let (status, _) = t.h.get(&t.b, &format!("/projects/{second}")).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the grant is for one project"
    );
    // A guest does not see the workspace's other projects in the list either.
    let (_, listed) = t.h.json(&t.b, Method::GET, "/projects", "").await;
    let ids: Vec<_> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(ids.contains(&t.a_project.to_string()), "{ids:?}");
    assert!(!ids.contains(&second.to_string()), "{ids:?}");
}

#[tokio::test]
async fn every_permission_named_in_the_table_exists_and_scopes_to_a_project() {
    for row in project_rows() {
        let Rule::Require(permission, _) = row.rule else {
            unreachable!()
        };
        assert_eq!(
            permission.scope(),
            deepref_domain::authorization::ResourceKind::Project,
            "{} {} requires a workspace permission on a project",
            row.method.as_str(),
            row.path
        );
        assert!(Permission::ALL.contains(&permission));
    }
}

#[tokio::test]
async fn head_on_a_foreign_project_is_the_same_as_on_a_missing_one() {
    let Some(t) = two_workspaces().await else {
        return;
    };
    let mut checked = 0;
    for row in project_rows()
        .into_iter()
        .filter(|row| row.method == Verb::Get)
    {
        let foreign = concrete(row.path, t.a_project);
        let missing = foreign.replace(&t.a_project.to_string(), &Uuid::new_v4().to_string());
        let (foreign_status, foreign_body) = t.h.send(&t.b, Method::HEAD, &foreign, None).await;
        let (missing_status, missing_body) = t.h.send(&t.b, Method::HEAD, &missing, None).await;
        assert_eq!(foreign_status, StatusCode::NOT_FOUND, "HEAD {}", row.path);
        assert_eq!(foreign_status, missing_status, "HEAD {}", row.path);
        assert_eq!(foreign_body, missing_body, "HEAD {}", row.path);
        checked += 1;
    }
    assert!(checked > 60, "{checked}");
    // The owner's HEAD still reaches the handler.
    let (status, _) =
        t.h.send(
            &t.a,
            Method::HEAD,
            &format!("/projects/{}", t.a_project),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

/// Every method against every registered project path, as a foreign principal:
/// a method the table lists is denied exactly like a missing project, and any
/// other method is the router's 405. Nothing is ever served. This is the guard
/// for routes or methods registered outside the OpenAPI router.
#[tokio::test]
async fn every_method_on_every_project_path_is_denied_or_not_allowed() {
    let Some(t) = two_workspaces().await else {
        return;
    };
    let mut paths: Vec<&str> = project_rows().iter().map(|row| row.path).collect();
    paths.sort_unstable();
    paths.dedup();
    let methods = [
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
    ];
    let mut failures = Vec::new();
    for path in &paths {
        for method in &methods {
            let foreign = concrete(path, t.a_project);
            let missing = foreign.replace(&t.a_project.to_string(), &Uuid::new_v4().to_string());
            let (status, body) = t.h.send(&t.b, method.clone(), &foreign, None).await;
            let (missing_status, missing_body) =
                t.h.send(&t.b, method.clone(), &missing, None).await;
            let lookup = if method == Method::HEAD {
                "GET"
            } else {
                method.as_str()
            };
            let listed = deepref_http_api::authz::policy::lookup(lookup, path).is_some();
            let ok = if *method == Method::OPTIONS {
                // Answered by the CORS layer outside the PEP: no handler, no body.
                body.is_empty() && body == missing_body && status == missing_status
            } else if listed {
                status == StatusCode::NOT_FOUND
                    && status == missing_status
                    && normalize(&body) == normalize(&missing_body)
            } else {
                status == StatusCode::METHOD_NOT_ALLOWED && missing_status == status
            };
            if !ok {
                failures.push(format!(
                    "{method} {path}: {status} (missing: {missing_status}), listed={listed}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn query_project_routes_are_indistinguishable_from_a_missing_one() {
    let Some(t) = two_workspaces().await else {
        return;
    };
    for row in ROUTES
        .iter()
        .filter(|row| matches!(row.rule, Rule::RequireIfPresent(..)))
    {
        let foreign = format!("{}?project_id={}", row.path, t.a_project);
        let missing = format!("{}?project_id={}", row.path, Uuid::new_v4());
        let garbage = format!("{}?project_id=not-a-uuid", row.path);
        let (fs, fb) = t.h.send(&t.b, method(row.method), &foreign, None).await;
        let (ms, mb) = t.h.send(&t.b, method(row.method), &missing, None).await;
        let (gs, gb) = t.h.send(&t.b, method(row.method), &garbage, None).await;
        assert_eq!(fs, StatusCode::NOT_FOUND, "{}", row.path);
        assert_eq!((fs, normalize(&fb)), (ms, normalize(&mb)), "{}", row.path);
        assert_eq!((fs, normalize(&fb)), (gs, normalize(&gb)), "{}", row.path);
        // Without the parameter no project data is returned.
        let (status, body) = t.h.get(&t.b, row.path).await;
        assert_eq!(status, StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(body["budget"].is_null(), "{body}");
        // The owner gets their own project's data.
        let own = format!("{}?project_id={}", row.path, t.a_project);
        let (status, body) = t.h.get(&t.a, &own).await;
        assert_eq!(status, StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(body["budget"].is_object(), "{body}");
    }
}

#[test]
fn no_route_is_registered_outside_the_openapi_router() {
    // The only `.route(` is the `/openapi.json` document itself; every other
    // operation goes through `routes!`, which the OpenAPI-derived check sees.
    fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for needle in [
                    ".route(",
                    ".route_service(",
                    ".nest(",
                    ".nest_service(",
                    ".fallback",
                    ".route_layer(",
                ] {
                    // Test modules build throwaway routers; only production code counts.
                    let production = text.split("#[cfg(test)]").next().unwrap();
                    for _ in production.matches(needle) {
                        found.push(format!("{}: {needle}", path.display()));
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    found.retain(|entry| !entry.contains("routes/mod.rs: .route("));
    assert!(
        found.is_empty(),
        "undocumented route registration: {found:?}"
    );
}
