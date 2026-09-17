use std::{future::Future, pin::Pin};

use deepref_domain::{Actor, ProjectId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AgentTool, AgentToolError, AgentToolName, AuthorityTier, PolicyDecision, PolicyEngine,
    PolicyInput, ProjectAiPolicy, RequestedAction,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantRole {
    System,
    User,
    Assistant,
    Tool,
}

impl AssistantRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Self::System),
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "tool" => Some(Self::Tool),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantToolCall {
    pub id: String,
    pub tool: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantToolResult {
    pub tool_call_id: String,
    pub tool: String,
    pub output: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal_review_run_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantChatMessage {
    pub role: AssistantRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<AssistantToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_results: Option<Vec<AssistantToolResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub fn assistant_tool_declarations() -> Vec<ToolDeclaration> {
    vec![
        ToolDeclaration {
            name: "get_project_protocol".to_owned(),
            description:
                "Retrieve published protocol criteria and review objectives for the project."
                    .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" }
                },
                "required": ["project_id"]
            }),
        },
        ToolDeclaration {
            name: "get_report".to_owned(),
            description: "Retrieve bibliographic metadata and acquisition details for a report."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "report_id": { "type": "string", "format": "uuid", "description": "Report identifier" }
                },
                "required": ["project_id", "report_id"]
            }),
        },
        ToolDeclaration {
            name: "read_document_blocks".to_owned(),
            description: "Read parsed full-text document blocks with page and section references."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "document_id": { "type": "string", "format": "uuid", "description": "Document identifier" },
                    "block_ids": {
                        "type": "array",
                        "items": { "type": "string", "format": "uuid" },
                        "description": "Specific block IDs to read"
                    }
                },
                "required": ["project_id", "document_id", "block_ids"]
            }),
        },
        ToolDeclaration {
            name: "search_document".to_owned(),
            description: "Search document text blocks by query with bounded limits.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "document_id": { "type": "string", "format": "uuid", "description": "Document identifier" },
                    "query": { "type": "string", "description": "Search query text" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Maximum blocks to return" }
                },
                "required": ["project_id", "document_id", "query", "limit"]
            }),
        },
        ToolDeclaration {
            name: "search_project_reports".to_owned(),
            description:
                "Search bibliographic reports across the project by title, abstract, or authors."
                    .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "query": { "type": "string", "description": "Search query terms" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Maximum reports to return" }
                },
                "required": ["project_id", "query", "limit"]
            }),
        },
        ToolDeclaration {
            name: "get_screening_state".to_owned(),
            description: "Inspect the current screening status and decisions for a report."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "report_id": { "type": "string", "format": "uuid", "description": "Report identifier" }
                },
                "required": ["project_id", "report_id"]
            }),
        },
        ToolDeclaration {
            name: "get_study".to_owned(),
            description:
                "Retrieve study details, associated reports, and synthesis classifications."
                    .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "study_id": { "type": "string", "format": "uuid", "description": "Study identifier" }
                },
                "required": ["project_id", "study_id"]
            }),
        },
        ToolDeclaration {
            name: "get_appraisal".to_owned(),
            description:
                "Inspect critical appraisal answers and risk-of-bias judgments for a report."
                    .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "report_id": { "type": "string", "format": "uuid", "description": "Report identifier" },
                    "definition_id": { "type": "string", "description": "Appraisal framework definition key" },
                    "definition_version": { "type": "integer", "description": "Framework version" }
                },
                "required": ["project_id", "report_id", "definition_id", "definition_version"]
            }),
        },
        ToolDeclaration {
            name: "propose_screening_decision".to_owned(),
            description:
                "Propose an inclusion or exclusion screening decision into the human review queue."
                    .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "report_id": { "type": "string", "format": "uuid", "description": "Report identifier" },
                    "stage": { "type": "string", "enum": ["title_abstract", "full_text"], "description": "Screening stage" }
                },
                "required": ["project_id", "report_id", "stage"]
            }),
        },
        ToolDeclaration {
            name: "propose_duplicate_merge".to_owned(),
            description:
                "Propose merging duplicate bibliographic records into the human review queue."
                    .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "source_record_id": { "type": "string", "format": "uuid", "description": "Source record identifier" },
                    "candidate_report_id": { "type": "string", "format": "uuid", "description": "Candidate report identifier" }
                },
                "required": ["project_id", "source_record_id", "candidate_report_id"]
            }),
        },
        ToolDeclaration {
            name: "propose_study_grouping".to_owned(),
            description: "Propose grouping multiple reports under a common study for review."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "report_id": { "type": "string", "format": "uuid", "description": "Report identifier" }
                },
                "required": ["project_id", "report_id"]
            }),
        },
        ToolDeclaration {
            name: "propose_classification".to_owned(),
            description: "Propose study design classification into the review queue.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "study_id": { "type": "string", "format": "uuid", "description": "Study identifier" }
                },
                "required": ["project_id", "study_id"]
            }),
        },
        ToolDeclaration {
            name: "propose_extraction".to_owned(),
            description: "Propose data extraction field values into the human review queue."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "study_id": { "type": "string", "format": "uuid", "description": "Study identifier" }
                },
                "required": ["project_id", "study_id"]
            }),
        },
        ToolDeclaration {
            name: "propose_appraisal_answer".to_owned(),
            description: "Propose critical appraisal answers into the human review queue."
                .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "report_id": { "type": "string", "format": "uuid", "description": "Report identifier" },
                    "definition_id": { "type": "string", "description": "Appraisal framework definition key" },
                    "definition_version": { "type": "integer", "description": "Framework version" }
                },
                "required": ["project_id", "report_id", "definition_id", "definition_version"]
            }),
        },
        ToolDeclaration {
            name: "trigger_workflow".to_owned(),
            description:
                "Trigger an asynchronous background automation recipe workflow for the project."
                    .to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "project_id": { "type": "string", "format": "uuid", "description": "Project identifier" },
                    "definition_id": { "type": "string", "format": "uuid", "description": "Automation definition UUID" },
                    "parameters": { "type": "object", "description": "Workflow execution parameters" }
                },
                "required": ["project_id", "definition_id"]
            }),
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AssistantToolOutput {
    Read(Value),
    Proposal {
        review_run_id: Uuid,
        status_path: String,
    },
    WorkflowTriggered {
        run_id: Uuid,
        job_id: Uuid,
        created: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum AssistantStreamEvent {
    Token {
        delta: String,
    },
    ToolStart {
        tool: String,
        tool_call_id: String,
        args: Value,
    },
    ToolComplete {
        tool: String,
        tool_call_id: String,
        output: Value,
    },
    ProposalCreated {
        tool: String,
        review_run_id: Uuid,
        status_path: String,
    },
    Done {
        message_id: Uuid,
        input_tokens: u64,
        output_tokens: u64,
    },
}

pub fn authorize_assistant_tool(
    policy_engine: &PolicyEngine,
    project_id: ProjectId,
    project_policy: &ProjectAiPolicy,
    actor: &Actor,
    tool_name: &str,
    args: &Value,
) -> Result<PolicyDecision, AgentToolError> {
    if tool_name == "trigger_workflow" {
        let declared_project_id = args
            .get("project_id")
            .and_then(Value::as_str)
            .and_then(|id_str| Uuid::parse_str(id_str).ok())
            .map(ProjectId::new)
            .ok_or(AgentToolError::InvalidProjectScope)?;
        if declared_project_id != project_id {
            return Err(AgentToolError::InvalidProjectScope);
        }

        let input = PolicyInput {
            actor: actor.clone(),
            project_id,
            declared_project_id,
            tool: tool_name.to_owned(),
            action: RequestedAction::WorkflowSuggestion,
            authority: AuthorityTier::WorkflowSuggestion,
            args: args.clone(),
            project_policy: project_policy.clone(),
        };
        let decision = policy_engine.authorize(&input);
        if decision == PolicyDecision::Forbidden {
            return Err(AgentToolError::Forbidden);
        }
        return Ok(decision);
    }

    let parsed_name = AgentToolName::parse(tool_name).ok_or(AgentToolError::UnknownTool)?;
    let tool = AgentTool::from_name_and_args(parsed_name, args.clone())
        .map_err(|_| AgentToolError::MalformedRequest)?;
    tool.validate()?;
    let declared_project_id = tool.project_id();
    if declared_project_id != project_id {
        return Err(AgentToolError::InvalidProjectScope);
    }

    let input = PolicyInput {
        actor: actor.clone(),
        project_id,
        declared_project_id,
        tool: parsed_name.as_str().to_owned(),
        action: if parsed_name.is_read() {
            RequestedAction::Read
        } else {
            RequestedAction::ScientificConclusion
        },
        authority: if parsed_name.is_read() {
            AuthorityTier::ReadOnly
        } else {
            AuthorityTier::ScientificConclusion
        },
        args: args.clone(),
        project_policy: project_policy.clone(),
    };

    let decision = policy_engine.authorize(&input);
    if decision == PolicyDecision::Forbidden {
        return Err(AgentToolError::Forbidden);
    }
    Ok(decision)
}

pub trait AssistantDispatcher: Send + Sync {
    fn execute_tool<'a>(
        &'a self,
        tool_name: &'a str,
        args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantToolOutput, AgentToolError>> + Send + 'a>>;
}

pub struct AssistantTurnInput<'a> {
    pub project_id: ProjectId,
    pub actor: Actor,
    pub project_policy: ProjectAiPolicy,
    pub history: &'a [AssistantChatMessage],
    pub user_message: &'a str,
}

pub async fn run_assistant_react_turn<D>(
    input: AssistantTurnInput<'_>,
    dispatcher: &D,
    event_tx: &tokio::sync::mpsc::Sender<AssistantStreamEvent>,
) -> Result<AssistantChatMessage, AgentToolError>
where
    D: AssistantDispatcher,
{
    let policy_engine = PolicyEngine;

    // Detect if user message is a structured tool command
    let parsed_json_tool = serde_json::from_str::<Value>(input.user_message).ok();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();

    if let Some(tool_name) = parsed_json_tool
        .as_ref()
        .and_then(|json_val| json_val.get("tool"))
        .and_then(Value::as_str)
    {
        let args = parsed_json_tool
            .as_ref()
            .and_then(|json_val| json_val.get("args"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        tool_calls.push(AssistantToolCall {
            id: Uuid::new_v4().to_string(),
            tool: tool_name.to_owned(),
            args,
        });
    }

    for call in &tool_calls {
        authorize_assistant_tool(
            &policy_engine,
            input.project_id,
            &input.project_policy,
            &input.actor,
            &call.tool,
            &call.args,
        )?;

        let _ = event_tx
            .send(AssistantStreamEvent::ToolStart {
                tool: call.tool.clone(),
                tool_call_id: call.id.clone(),
                args: call.args.clone(),
            })
            .await;

        let output = dispatcher
            .execute_tool(&call.tool, call.args.clone())
            .await?;

        let (result_value, review_run_id) = match output {
            AssistantToolOutput::Read(val) => (val, None),
            AssistantToolOutput::Proposal {
                review_run_id,
                status_path,
            } => {
                let _ = event_tx
                    .send(AssistantStreamEvent::ProposalCreated {
                        tool: call.tool.clone(),
                        review_run_id,
                        status_path: status_path.clone(),
                    })
                    .await;
                (
                    json!({
                        "review_run_id": review_run_id,
                        "status_path": status_path,
                    }),
                    Some(review_run_id),
                )
            }
            AssistantToolOutput::WorkflowTriggered {
                run_id,
                job_id,
                created,
            } => (
                json!({
                    "run_id": run_id,
                    "job_id": job_id,
                    "created": created,
                }),
                None,
            ),
        };

        let _ = event_tx
            .send(AssistantStreamEvent::ToolComplete {
                tool: call.tool.clone(),
                tool_call_id: call.id.clone(),
                output: result_value.clone(),
            })
            .await;

        tool_results.push(AssistantToolResult {
            tool_call_id: call.id.clone(),
            tool: call.tool.clone(),
            output: result_value,
            proposal_review_run_id: review_run_id,
        });
    }

    let final_answer = if !tool_results.is_empty() {
        if let Some(first_res) = tool_results.first() {
            if let Some(run_id) = first_res.proposal_review_run_id {
                format!(
                    "I have submitted the requested proposal to the review queue for human sign-off (review run {}). Scientific truth remains protected until verified.",
                    run_id
                )
            } else if first_res.tool == "trigger_workflow" {
                "The background automation workflow has been scheduled successfully.".to_owned()
            } else {
                format!(
                    "Tool execution completed successfully for `{}`.",
                    first_res.tool
                )
            }
        } else {
            "Action completed.".to_owned()
        }
    } else {
        "Hello! I am DeepRef's AI assistant. I can inspect protocol criteria, read full text document blocks, propose screening/deduplication/appraisal reviews, and trigger background automations. How can I assist your systematic review today?".to_owned()
    };

    for chunk in final_answer.split_inclusive(' ') {
        let _ = event_tx
            .send(AssistantStreamEvent::Token {
                delta: chunk.to_owned(),
            })
            .await;
    }

    let message_id = Uuid::new_v4();
    let input_tokens = (input.user_message.chars().count() / 4) as u64;
    let output_tokens = (final_answer.chars().count() / 4) as u64;
    let _ = event_tx
        .send(AssistantStreamEvent::Done {
            message_id,
            input_tokens,
            output_tokens,
        })
        .await;

    Ok(AssistantChatMessage {
        role: AssistantRole::Assistant,
        content: final_answer,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        tool_results: if tool_results.is_empty() {
            None
        } else {
            Some(tool_results)
        },
        metadata: Some(json!({
            "message_id": message_id,
            "system_invariants": "scientific_truth_requires_human_sign_off",
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepref_domain::{ActorKind, ProjectId};
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct MockDispatcher {
        executed: Arc<Mutex<Vec<(String, Value)>>>,
    }

    impl AssistantDispatcher for MockDispatcher {
        fn execute_tool<'a>(
            &'a self,
            tool_name: &'a str,
            args: Value,
        ) -> Pin<Box<dyn Future<Output = Result<AssistantToolOutput, AgentToolError>> + Send + 'a>>
        {
            let executed = Arc::clone(&self.executed);
            let name = tool_name.to_owned();
            Box::pin(async move {
                executed.lock().unwrap().push((name.clone(), args));
                if name.starts_with("propose_") {
                    Ok(AssistantToolOutput::Proposal {
                        review_run_id: Uuid::from_u128(0xabcd),
                        status_path: "/projects/test/review/runs/123".to_owned(),
                    })
                } else if name == "trigger_workflow" {
                    Ok(AssistantToolOutput::WorkflowTriggered {
                        run_id: Uuid::from_u128(0x1),
                        job_id: Uuid::from_u128(0x2),
                        created: true,
                    })
                } else {
                    Ok(AssistantToolOutput::Read(json!({"read": "ok"})))
                }
            })
        }
    }

    #[test]
    fn test_all_15_tool_declarations_present() {
        let decls = assistant_tool_declarations();
        assert_eq!(decls.len(), 15);
        let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"get_project_protocol"));
        assert!(names.contains(&"propose_screening_decision"));
        assert!(names.contains(&"trigger_workflow"));
    }

    #[test]
    fn test_policy_authorization_rules() {
        let policy_engine = PolicyEngine;
        let project_id = ProjectId::new(Uuid::from_u128(1));
        let other_project_id = ProjectId::new(Uuid::from_u128(2));
        let policy = ProjectAiPolicy::default();
        let actor = Actor::new(ActorKind::User, "researcher-1").unwrap();

        // 1. Read tool authorized
        let read_args = json!({ "project_id": project_id.as_uuid().to_string() });
        let decision = authorize_assistant_tool(
            &policy_engine,
            project_id,
            &policy,
            &actor,
            "get_project_protocol",
            &read_args,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::ExecuteRead);

        // 2. Proposal tool creates proposal
        let prop_args = json!({
            "project_id": project_id.as_uuid().to_string(),
            "report_id": Uuid::from_u128(10).to_string(),
            "stage": "title_abstract"
        });
        let decision = authorize_assistant_tool(
            &policy_engine,
            project_id,
            &policy,
            &actor,
            "propose_screening_decision",
            &prop_args,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::CreateProposal);

        // 3. Cross project is forbidden
        let cross_args = json!({ "project_id": other_project_id.as_uuid().to_string() });
        let err = authorize_assistant_tool(
            &policy_engine,
            project_id,
            &policy,
            &actor,
            "get_project_protocol",
            &cross_args,
        )
        .unwrap_err();
        assert_eq!(err, AgentToolError::InvalidProjectScope);

        // 4. trigger_workflow authorized
        let wf_args = json!({
            "project_id": project_id.as_uuid().to_string(),
            "definition_id": Uuid::from_u128(99).to_string()
        });
        let decision = authorize_assistant_tool(
            &policy_engine,
            project_id,
            &policy,
            &actor,
            "trigger_workflow",
            &wf_args,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::CreateProposal);

        // 5. Unknown tool rejected
        let err = authorize_assistant_tool(
            &policy_engine,
            project_id,
            &policy,
            &actor,
            "drop_database",
            &read_args,
        )
        .unwrap_err();
        assert_eq!(err, AgentToolError::UnknownTool);
    }

    #[tokio::test]
    async fn test_assistant_react_turn_streaming() {
        let project_id = ProjectId::new(Uuid::from_u128(1));
        let actor = Actor::new(ActorKind::User, "test-user").unwrap();
        let dispatcher = MockDispatcher::default();
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);

        let input = AssistantTurnInput {
            project_id,
            actor,
            project_policy: ProjectAiPolicy::default(),
            history: &[],
            user_message: "Hello assistant",
        };

        let response = run_assistant_react_turn(input, &dispatcher, &tx)
            .await
            .unwrap();

        assert_eq!(response.role, AssistantRole::Assistant);
        assert!(!response.content.is_empty());

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AssistantStreamEvent::Token { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AssistantStreamEvent::Done { .. }))
        );
    }
}
