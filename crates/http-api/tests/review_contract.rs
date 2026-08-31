#[test]
fn v2_review_contract_is_exposed_in_openapi() {
    let document = serde_json::to_value(deepref_http_api::routes::openapi_document()).unwrap();
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

#[test]
fn deduplication_contract_is_exposed_in_openapi() {
    let document = serde_json::to_value(deepref_http_api::routes::openapi_document()).unwrap();
    let paths = document["paths"].as_object().unwrap();
    for path in [
        "/projects/{project_id}/deduplication/run",
        "/projects/{project_id}/deduplication/proposals",
        "/projects/{project_id}/deduplication/proposals/{proposal_id}/decision",
        "/projects/{project_id}/records/{record_id}/resolution",
    ] {
        assert!(
            paths.contains_key(path),
            "missing deduplication path {path}"
        );
    }
    let schemas = document["components"]["schemas"].as_object().unwrap();
    assert!(schemas.contains_key("DedupeProposalDto"));
    assert!(schemas.contains_key("ProposalDecisionRequest"));
}
