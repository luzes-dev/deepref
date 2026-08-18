#[test]
fn health_contract_is_in_openapi() {
    let paths = deepref_api::routes::openapi_document().paths.paths;
    for path in [
        "/health/live",
        "/health/ready",
        "/health/dependencies",
        "/health",
    ] {
        assert!(paths.contains_key(path), "missing {path}");
    }
}
