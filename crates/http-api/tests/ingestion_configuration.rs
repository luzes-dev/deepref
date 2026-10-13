#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
#[test]
fn acquisition_configuration_failure_is_documented_and_ingestions_are_retired() {
    let document = serde_json::to_value(deepref_http_api::routes::openapi_document()).unwrap();
    assert!(document["paths"]["/projects/{project_id}/acquisitions"]["post"]["responses"]["400"].is_object());
    assert!(document["paths"].get("/ingestions").is_none());
}
