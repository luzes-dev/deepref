#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use serde_json::Value;

#[test]
fn automation_paths_and_operation_ids_are_exposed() {
    let document: Value =
        serde_json::to_value(deepref_http_api::routes::openapi_document()).expect("OpenAPI JSON");
    let operations = [
        (
            "/projects/{project_id}/automations/definitions",
            "get",
            "listAutomationDefinitions",
        ),
        (
            "/projects/{project_id}/automations/definitions/{recipe}",
            "put",
            "configureAutomationDefinition",
        ),
        (
            "/projects/{project_id}/automations/runs",
            "post",
            "triggerAutomationManually",
        ),
        (
            "/projects/{project_id}/automations/runs",
            "get",
            "listAutomationRuns",
        ),
        (
            "/projects/{project_id}/automations/runs/{run_id}",
            "get",
            "getAutomationRun",
        ),
    ];

    for (path, method, operation_id) in operations {
        assert_eq!(
            document["paths"][path][method]["operationId"], operation_id,
            "missing or incorrect operationId for {method} {path}"
        );
    }
}

#[test]
fn automation_openapi_contract_is_closed_and_bounded() {
    let document: Value =
        serde_json::to_value(deepref_http_api::routes::openapi_document()).expect("OpenAPI JSON");
    let configure =
        &document["paths"]["/projects/{project_id}/automations/definitions/{recipe}"]["put"];
    assert_eq!(
        configure["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ConfigureAutomationRequest"
    );
    let trigger = &document["paths"]["/projects/{project_id}/automations/runs"]["post"];
    assert!(trigger["parameters"].as_array().is_some_and(|parameters| {
        parameters.iter().any(|parameter| {
            parameter["name"] == "Idempotency-Key"
                && parameter["in"] == "header"
                && parameter["required"] == true
        })
    }));
    let schemas = &document["components"]["schemas"];
    assert_eq!(
        schemas["AutomationTriggerInput"]["enum"],
        serde_json::json!([
            "report_added",
            "acquisition_completed",
            "full_text_attached",
            "report_included",
            "study_created",
            "appraisal_completed",
            "manual"
        ])
    );
    assert_eq!(
        schemas["AutomationDefinitionStatusInput"]["enum"],
        serde_json::json!(["active", "paused"])
    );
    let list_runs = &document["paths"]["/projects/{project_id}/automations/runs"]["get"];
    assert!(
        list_runs["parameters"]
            .as_array()
            .is_some_and(|parameters| {
                parameters.iter().any(|parameter| {
                    parameter["name"] == "limit"
                        && parameter["in"] == "query"
                        && parameter["schema"]["type"]
                            .as_array()
                            .is_some_and(|types| types.iter().any(|kind| kind == "integer"))
                })
            })
    );
    assert!(trigger["responses"]["200"].is_object());
    assert!(trigger["responses"]["201"].is_object());
    assert!(trigger["responses"]["409"].is_object());
}
