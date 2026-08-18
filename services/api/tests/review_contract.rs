#[test]
fn v2_review_contract_is_exposed_in_openapi() {
    let document = serde_json::to_value(deepref_api::routes::openapi_document()).unwrap();
    let paths = document["paths"].as_object().unwrap();
    for path in [
        "/projects/{project_id}/protocol",
        "/projects/{project_id}/screening/title-abstract",
        "/projects/{project_id}/reports/{report_id}/screening",
        "/projects/{project_id}/prisma",
    ] {
        assert!(paths.contains_key(path), "missing v2 path {path}");
    }
    assert!(
        document["components"]["schemas"]
            .as_object()
            .unwrap()
            .contains_key("ScreenReportRequest")
    );
}
