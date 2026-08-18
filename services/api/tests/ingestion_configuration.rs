#[test]
fn ingestion_configuration_failure_is_documented() {
    let document = serde_json::to_value(deepref_api::routes::openapi_document()).unwrap();
    assert!(document["paths"]["/ingestions"]["post"]["responses"]["400"].is_object());
}
