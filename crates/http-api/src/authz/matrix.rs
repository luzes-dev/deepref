//! Renders the policy table as `docs/authorization-matrix.md` and compares it
//! with the routes the API actually registers.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use super::policy::{
    HumanActivity, NON_HTTP, ROUTES, ResourceFrom, RouteRule, Rule, Verb, human_activity,
    machine_safe,
};

/// A route registered outside the OpenAPI router in `routes::router`.
const UNDOCUMENTED_ROUTES: &[(Verb, &str)] = &[(Verb::Get, "/openapi.json")];

pub type RouteKey = (String, String);

/// Every (method, path) the live router serves.
pub fn router_routes() -> BTreeSet<RouteKey> {
    let document = crate::routes::openapi_document_json().unwrap_or_default();
    let mut routes = openapi_routes(&document);
    routes.extend(
        UNDOCUMENTED_ROUTES
            .iter()
            .map(|(verb, path)| (verb.as_str().to_owned(), (*path).to_owned())),
    );
    routes
}

/// Every (method, path) an OpenAPI document declares.
pub fn openapi_routes(document: &serde_json::Value) -> BTreeSet<RouteKey> {
    let mut routes = BTreeSet::new();
    let Some(paths) = document.get("paths").and_then(serde_json::Value::as_object) else {
        return routes;
    };
    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for method in item.keys() {
            if let Some(verb) = Verb::parse(method) {
                routes.insert((verb.as_str().to_owned(), path.clone()));
            }
        }
    }
    routes
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// A route the API serves with no policy row: it would be denied.
    RouteWithoutRow(RouteKey),
    /// A policy row for a route the API does not serve.
    RowWithoutRoute(RouteKey),
    /// Two rows for one route.
    DuplicateRow(RouteKey),
    /// No reviewed activity semantics for a current write operation.
    ActivityUnclassified(RouteKey),
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RouteWithoutRow((m, p)) => {
                write!(f, "route {m} {p} has no row in the policy table")
            }
            Self::RowWithoutRoute((m, p)) => write!(f, "policy row {m} {p} matches no route"),
            Self::DuplicateRow((m, p)) => write!(f, "policy table has two rows for {m} {p}"),
            Self::ActivityUnclassified((m, p)) => {
                write!(f, "route {m} {p} has no human activity classification")
            }
        }
    }
}

/// Compares a route set with a table. Empty means they agree exactly.
pub fn compare(routes: &BTreeSet<RouteKey>, table: &[RouteRule]) -> Vec<Mismatch> {
    let mut rows = BTreeSet::new();
    let mut mismatches = Vec::new();
    for row in table {
        let key = (row.method.as_str().to_owned(), row.path.to_owned());
        if !rows.insert(key.clone()) {
            mismatches.push(Mismatch::DuplicateRow(key));
        }
    }
    mismatches.extend(
        routes
            .difference(&rows)
            .cloned()
            .map(Mismatch::RouteWithoutRow),
    );
    mismatches.extend(
        rows.difference(routes)
            .cloned()
            .map(Mismatch::RowWithoutRoute),
    );
    mismatches
}

/// Mismatches of the real table against the live router.
pub fn router_mismatches() -> Vec<Mismatch> {
    let mut mismatches = compare(&router_routes(), ROUTES);
    mismatches.extend(
        ROUTES
            .iter()
            .filter(|row| human_activity(row) == HumanActivity::Unclassified)
            .map(|row| {
                Mismatch::ActivityUnclassified((
                    row.method.as_str().to_owned(),
                    row.path.to_owned(),
                ))
            }),
    );
    mismatches
}

fn rule_columns(rule: Rule) -> (&'static str, String, String) {
    match rule {
        Rule::Open => ("Open", "-".to_owned(), "-".to_owned()),
        Rule::Authenticated => ("Authenticated", "-".to_owned(), "user-scoped".to_owned()),
        Rule::Require(permission, ResourceFrom::PathParam(name)) => (
            "Require",
            format!("`{}`", permission.as_str()),
            format!("project (`{{{name}}}`)"),
        ),
        Rule::RequireIfPresent(permission, ResourceFrom::Query(name)) => (
            "RequireIfPresent",
            format!("`{}`", permission.as_str()),
            format!("project (query `{name}`, when given)"),
        ),
        Rule::Require(permission, ResourceFrom::Query(name)) => (
            "Require",
            format!("`{}`", permission.as_str()),
            format!("project (query `{name}`)"),
        ),
        Rule::RequireIfPresent(permission, ResourceFrom::PathParam(name)) => (
            "RequireIfPresent",
            format!("`{}`", permission.as_str()),
            format!("project (`{{{name}}}`, when given)"),
        ),
        Rule::Handler(permission) => (
            "Handler",
            format!("`{}`", permission.as_str()),
            "workspace (request body)".to_owned(),
        ),
        Rule::PlatformOperator => ("PlatformOperator", "-".to_owned(), "platform".to_owned()),
    }
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

/// The checked-in matrix, generated from the table.
pub fn render_markdown() -> String {
    let mut out = String::new();
    out.push_str(
        "<!-- Generated by `cargo run -q -p deepref-http-api --bin authz_matrix`; do not edit. -->\n\
         # Authorization coverage matrix\n\n\
         Source of truth: `crates/http-api/src/authz/policy.rs`. The Policy Enforcement Point \
         (`crates/http-api/src/authz/mod.rs`) decides every request from these rows, denies a \
         route that has none, and `cargo xtask authz-matrix` fails when this file, the OpenAPI \
         document or the router disagree with the table.\n\n\
         A foreign-workspace principal calling a `Require` row receives exactly the response \
         for a nonexistent resource (`404 NOT_FOUND`).\n\n",
    );
    out.push_str("API keys are accepted only on these explicit machine-safe reads; all other routes require a human session or their documented open boundary:\n\n");
    for row in ROUTES.iter().filter(|row| machine_safe(row)) {
        let _ = writeln!(
            out,
            "- `{} {}` (central scope and live membership check)",
            row.method.as_str(),
            row.path
        );
    }
    out.push('\n');
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for row in ROUTES {
        *counts.entry(rule_columns(row.rule).0).or_default() += 1;
    }
    out.push_str("## Summary\n\n| Rule | Rows |\n| --- | --- |\n");
    for (rule, count) in &counts {
        let _ = writeln!(out, "| {rule} | {count} |");
    }
    let _ = writeln!(out, "| **Total HTTP** | **{}** |", ROUTES.len());
    let _ = writeln!(out, "| **Non-HTTP paths** | **{}** |\n", NON_HTTP.len());
    out.push_str("Human activity receipts denote authenticated attempts before dispatch, never business success or a permission grant. Reads, derived queries, acknowledgements, machine deliveries and operator work are excluded. Unclassified writes fail the matrix check and protected dispatch.\n\n");
    out.push_str("## HTTP routes\n\n| Method | Path | Rule | Permission | Resource | Human activity | Foreign-workspace test | Reason |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n");
    let mut rows: Vec<_> = ROUTES.iter().collect();
    rows.sort_by_key(|row| (row.path, row.method.as_str()));
    for row in rows {
        let (rule, permission, resource) = rule_columns(row.rule);
        let _ = writeln!(
            out,
            "| {} | `{}` | {} | {} | {} | {} | `{}` | {} |",
            row.method.as_str(),
            row.path,
            rule,
            permission,
            resource,
            human_activity(row).as_str(),
            cell(row.foreign_test),
            cell(row.reason),
        );
    }
    out.push_str("\n## Non-HTTP access paths\n\n| Kind | Path | Control | Test |\n| --- | --- | --- | --- |\n");
    for row in NON_HTTP {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            cell(row.kind),
            cell(row.name),
            cell(row.control),
            cell(row.test)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::policy::{FOREIGN_PROJECT_TEST, ResourceFrom};
    use deepref_domain::authorization::Permission;

    fn row(method: Verb, path: &'static str) -> RouteRule {
        RouteRule {
            method,
            path,
            rule: Rule::Require(
                Permission::ProjectView,
                ResourceFrom::PathParam("project_id"),
            ),
            foreign_test: FOREIGN_PROJECT_TEST,
            reason: "test",
        }
    }

    fn routes(items: &[(&str, &str)]) -> BTreeSet<RouteKey> {
        items
            .iter()
            .map(|(m, p)| ((*m).to_owned(), (*p).to_owned()))
            .collect()
    }

    #[test]
    fn a_route_without_a_row_is_reported() {
        let found = compare(
            &routes(&[
                ("GET", "/projects/{project_id}/a"),
                ("POST", "/projects/{project_id}/b"),
            ]),
            &[row(Verb::Get, "/projects/{project_id}/a")],
        );
        assert_eq!(
            found,
            vec![Mismatch::RouteWithoutRow((
                "POST".to_owned(),
                "/projects/{project_id}/b".to_owned()
            ))]
        );
    }

    #[test]
    fn a_row_without_a_route_is_reported() {
        let found = compare(
            &routes(&[("GET", "/projects/{project_id}/a")]),
            &[
                row(Verb::Get, "/projects/{project_id}/a"),
                row(Verb::Delete, "/projects/{project_id}/gone"),
            ],
        );
        assert_eq!(
            found,
            vec![Mismatch::RowWithoutRoute((
                "DELETE".to_owned(),
                "/projects/{project_id}/gone".to_owned()
            ))]
        );
    }

    #[test]
    fn the_same_path_with_another_method_is_a_different_route() {
        let found = compare(
            &routes(&[("GET", "/x"), ("PUT", "/x")]),
            &[row(Verb::Get, "/x")],
        );
        assert_eq!(
            found,
            vec![Mismatch::RouteWithoutRow((
                "PUT".to_owned(),
                "/x".to_owned()
            ))]
        );
    }

    #[test]
    fn duplicate_rows_are_reported() {
        let found = compare(
            &routes(&[("GET", "/x")]),
            &[row(Verb::Get, "/x"), row(Verb::Get, "/x")],
        );
        assert_eq!(
            found,
            vec![Mismatch::DuplicateRow(("GET".to_owned(), "/x".to_owned()))]
        );
    }

    #[test]
    fn the_real_table_covers_exactly_the_real_router() {
        let mismatches = router_mismatches();
        assert!(
            mismatches.is_empty(),
            "{}",
            mismatches
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn the_open_rows_are_exactly_the_session_layers_open_paths() {
        for row in ROUTES {
            let sample = row.path.replace("{token}", "0123abcd");
            assert_eq!(
                crate::auth::is_session_exempt(&sample),
                row.rule == Rule::Open,
                "{} {} disagrees with the session layer",
                row.method.as_str(),
                row.path
            );
        }
    }

    #[test]
    fn every_non_open_row_names_its_proof() {
        for row in ROUTES {
            if row.rule != Rule::Open && row.rule != Rule::Authenticated {
                assert_ne!(row.foreign_test, "n/a", "{}", row.path);
            }
            assert!(!row.reason.is_empty(), "{}", row.path);
        }
    }
}
