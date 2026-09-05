#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
#[test]
fn every_collection_operation_exposes_a_bounded_contract() {
    let document = serde_json::to_value(deepref_http_api::routes::openapi_document()).unwrap();
    for path in [
        "/projects",
        "/ingestions",
        "/ingestions/{ingestion_id}/items",
        "/projects/{project_id}/reports",
    ] {
        let get = &document["paths"][path]["get"];
        assert!(
            get["responses"]["200"].is_object(),
            "missing collection response for {path}"
        );
    }
}

#[test]
fn export_attachments_are_openapi_binary_and_not_json_or_arrays() {
    let document = serde_json::to_value(deepref_http_api::routes::openapi_document()).unwrap();
    let response = &document["paths"]["/projects/{project_id}/exports/{export_kind}"]["get"]["responses"]
        ["200"];
    let content = response["content"].as_object().expect("export content");
    assert!(!content.is_empty());
    for (media_type, media) in content {
        let schema = if let Some(name) = media["schema"]["$ref"]
            .as_str()
            .and_then(|reference| reference.strip_prefix("#/components/schemas/"))
        {
            &document["components"]["schemas"][name]
        } else {
            &media["schema"]
        };
        assert_eq!(schema["type"], "string", "{media_type} must be binary");
        assert_eq!(schema["format"], "binary", "{media_type} must be binary");
        assert_ne!(schema["type"], "array", "{media_type} must not be number[]");
    }
}
