#[test]
fn every_collection_operation_exposes_a_bounded_contract() {
    let document = serde_json::to_value(deepref_http_api::routes::openapi_document()).unwrap();
    for path in [
        "/projects",
        "/ingestions",
        "/ingestions/{ingestion_id}/items",
        "/projects/{project_id}/articles",
    ] {
        let get = &document["paths"][path]["get"];
        assert!(
            get["responses"]["200"].is_object(),
            "missing collection response for {path}"
        );
    }
}
