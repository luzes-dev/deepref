//! Renders or checks `docs/authorization-matrix.md`.
//!
//! * no arguments: prints the matrix to stdout (`pnpm generate:authz`).
//! * `--check <repo-root>`: fails when the table and the live router, the
//!   committed `docs/openapi.json`, or the committed matrix disagree.

use deepref_http_api::authz::matrix;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {
            print!("{}", matrix::render_markdown());
            Ok(())
        }
        [flag, root] if flag == "--check" => check(std::path::Path::new(root)),
        _ => anyhow::bail!("usage: authz_matrix [--check <repo-root>]"),
    }
}

fn check(root: &std::path::Path) -> anyhow::Result<()> {
    let mut problems: Vec<String> = matrix::router_mismatches()
        .iter()
        .map(|mismatch| format!("router: {mismatch}"))
        .collect();
    let openapi_path = root.join("docs/openapi.json");
    let openapi: serde_json::Value = serde_json::from_slice(&std::fs::read(&openapi_path)?)?;
    let mut documented = matrix::openapi_routes(&openapi);
    documented.insert(("GET".to_owned(), "/openapi.json".to_owned()));
    problems.extend(
        matrix::compare(&documented, deepref_http_api::authz::policy::ROUTES)
            .iter()
            .map(|mismatch| format!("docs/openapi.json: {mismatch}")),
    );
    let committed =
        std::fs::read_to_string(root.join("docs/authorization-matrix.md")).unwrap_or_default();
    if committed != matrix::render_markdown() {
        problems.push("docs/authorization-matrix.md is stale; run: pnpm generate:authz".to_owned());
    }
    if problems.is_empty() {
        println!("authorization matrix: ok");
        return Ok(());
    }
    for problem in &problems {
        eprintln!("{problem}");
    }
    anyhow::bail!(
        "authorization matrix check failed ({} problems)",
        problems.len()
    )
}
