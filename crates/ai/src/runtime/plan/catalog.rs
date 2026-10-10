//! The assistant tool catalog: what the model may call.
//!
//! Reads run immediately; every write-capable tool is intercepted and becomes
//! a [`PlanAction`][crate::PlanAction] a human must confirm (see
//! [`super::policy`]). The catalog maps one-to-one onto
//! [`assistant_tool_declarations`][crate::assistant_tool_declarations], minus
//! the host-owned `project_id` (injected by the adapter) and the workflow
//! trigger (never offered to the model).

use deepref_domain::ProjectId;
use serde_json::{Value, json};

use super::policy::MAX_REPORTS_PER_ACTION;
use crate::{AgentToolName, ToolDeclaration, assistant_tool_declarations};

/// Write tool that records title/abstract decisions for several reports.
pub const TOOL_SCREEN_REPORTS: &str = "screen_reports";
/// Never executable by the assistant; surfaced as a manual step.
pub const TOOL_FINAL_EXCLUSION: &str = "request_final_exclusion";
pub const TOOL_PUBLISH_PROTOCOL: &str = "request_protocol_publish";
pub const TOOL_PROJECT_OVERVIEW: &str = "get_project_overview";
pub const TOOL_LIST_REPORTS: &str = "list_reports_by_screening_status";

/// Version of the assistant system prompt, for cassette provenance.
pub const ASSISTANT_PROMPT_VERSION: &str = "assistant.agent.v2";

pub fn is_read_tool(name: &str) -> bool {
    name == TOOL_PROJECT_OVERVIEW
        || name == TOOL_LIST_REPORTS
        || AgentToolName::parse(name).is_some_and(AgentToolName::is_read)
}

pub(crate) fn is_write_tool(name: &str) -> bool {
    name == TOOL_SCREEN_REPORTS
        || name == TOOL_FINAL_EXCLUSION
        || name == TOOL_PUBLISH_PROTOCOL
        || AgentToolName::parse(name).is_some_and(AgentToolName::is_proposal)
}

fn strip_project_id(mut declaration: ToolDeclaration) -> ToolDeclaration {
    if let Some(properties) = declaration
        .parameters
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        properties.remove("project_id");
    }
    if let Some(required) = declaration
        .parameters
        .get_mut("required")
        .and_then(Value::as_array_mut)
    {
        required.retain(|name| name != "project_id");
    }
    declaration
}

fn as_write_tool(mut declaration: ToolDeclaration, kind: &str) -> ToolDeclaration {
    declaration.description = format!(
        "{} This does NOT run immediately: it is added to a plan the user must confirm. {kind}",
        declaration.description
    );
    if let Some(properties) = declaration
        .parameters
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        properties.insert(
            "summary".to_owned(),
            json!({"type": "string", "description": "One plain-language sentence saying what will be done, e.g. 'Exclude 14 records about animal studies'."}),
        );
        properties.insert(
            "rationale".to_owned(),
            json!({"type": "string", "description": "Why, in one or two sentences, grounded in the protocol or the data you read."}),
        );
    }
    if let Some(required) = declaration
        .parameters
        .get_mut("required")
        .and_then(Value::as_array_mut)
    {
        required.push(json!("summary"));
        required.push(json!("rationale"));
    }
    declaration
}

pub fn agent_tool_declarations() -> Vec<ToolDeclaration> {
    let mut tools = Vec::new();
    for declaration in assistant_tool_declarations() {
        let Some(name) = AgentToolName::parse(&declaration.name) else {
            continue; // trigger_workflow is intentionally not offered to the model
        };
        let declaration = strip_project_id(declaration);
        tools.push(if name.is_read() {
            declaration
        } else {
            as_write_tool(declaration, "")
        });
    }
    tools.push(ToolDeclaration {
        name: TOOL_PROJECT_OVERVIEW.to_owned(),
        description: "Counts of reports, studies and title/abstract screening status in this project. Call this first when the user asks about progress or what to do next.".to_owned(),
        parameters: json!({"type": "object", "properties": {}}),
    });
    tools.push(ToolDeclaration {
        name: TOOL_LIST_REPORTS.to_owned(),
        description: "List reports with their title/abstract screening status, optionally filtered. Returns report ids and titles (max 50).".to_owned(),
        parameters: json!({
            "type": "object",
            "properties": {
                "status": {"type": "string", "enum": ["unscreened", "include", "exclude", "maybe"]},
                "limit": {"type": "integer", "minimum": 1, "maximum": 50}
            }
        }),
    });
    tools.push(as_write_tool(
        ToolDeclaration {
            name: TOOL_SCREEN_REPORTS.to_owned(),
            description: "Record title/abstract screening decisions for specific reports on behalf of the user.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "report_ids": {"type": "array", "items": {"type": "string", "format": "uuid"}, "minItems": 1, "maxItems": MAX_REPORTS_PER_ACTION},
                    "decision": {"type": "string", "enum": ["include", "exclude", "maybe"]},
                    "notes": {"type": "string", "description": "Optional note stored with each decision."}
                },
                "required": ["report_ids", "decision"]
            }),
        },
        "Only for the title/abstract stage.",
    ));
    tools.push(as_write_tool(
        ToolDeclaration {
            name: TOOL_FINAL_EXCLUSION.to_owned(),
            description: "Use when the user wants a study finally excluded (full-text exclusion). You may NOT do this; it tells the user to do it themselves.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {"report_ids": {"type": "array", "items": {"type": "string", "format": "uuid"}}},
                "required": ["report_ids"]
            }),
        },
        "",
    ));
    tools.push(as_write_tool(
        ToolDeclaration {
            name: TOOL_PUBLISH_PROTOCOL.to_owned(),
            description: "Use when the user wants the protocol published or amended. You may NOT do this; it tells the user to do it themselves.".to_owned(),
            parameters: json!({"type": "object", "properties": {}}),
        },
        "",
    ));
    tools
}

pub fn assistant_system_prompt(project_id: ProjectId) -> String {
    format!(
        "You are DeepRef's assistant for a systematic literature review. Project id: {project_id}.\n\
         \n\
         Rules:\n\
         - Answer from data you read with tools; never invent report ids, titles, documents or counts.\n\
         - Read tools run immediately. Use them freely but economically.\n\
         - You cannot change anything directly. Every change tool only adds an item to a plan that the user must confirm. A change tool result with status queued_in_plan means the item is in the plan; any other result means it is not.\n\
         - Only say that something was queued, planned or is waiting for confirmation when a change tool returned queued_in_plan in this turn. If no change tool was called, say plainly that nothing was queued and nothing has changed. After queuing changes, say what the plan contains and that nothing has been changed yet.\n\
         - Final exclusion of studies and publishing/amending the protocol are never yours to do. If asked, call the matching request tool so the user is pointed to the right screen.\n\
         - Prefer one batched action (screen_reports with many report ids) over many single ones. Give each change a clear summary and rationale.\n\
         - Counts: use get_project_overview as it is. The title_abstract buckets add up to the report total. full_text.awaiting_decision is a part of title_abstract.include, not an extra bucket.\n\
         - Full text: a report has full text only when get_report lists a document with status available. A report without such a document has no full text, even when it is included.\n\
         - Searching: search_project_reports matches any of the words given and ranks the best matches first. Pass key terms, not a whole sentence. If nothing matches, try other key terms before saying that no record mentions it.\n\
         - Studies: use list_studies to find a study id. Never pass the project id where a study or report id is expected.\n\
         - Language: reply in the same language as the user's latest message, even when the protocol or the records are in another language. Quote records in their original language and explain in the user's language.\n\
         - Be concise and plain; the user is a researcher, not an engineer. Do not mention tool names, JSON or ids unless useful.\n\
         - If a tool returns an error, adapt or explain; do not retry the same call.",
        project_id = project_id.as_uuid()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_never_sees_workflow_trigger_or_project_id() {
        let tools = agent_tool_declarations();
        assert!(tools.iter().all(|tool| tool.name != "trigger_workflow"));
        assert!(
            tools
                .iter()
                .all(|tool| !tool.parameters.to_string().contains("project_id"))
        );
    }
}
