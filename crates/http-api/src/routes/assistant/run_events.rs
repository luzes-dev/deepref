//! Assistant durable run observation: reconnectable SSE over persisted run
//! events. The browser resumes with `after_seq`; the worker owns execution,
//! so a disconnect never cancels the run.

use axum::{
    extract::{Path, Query, State},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use deepref_ai::AssistantStreamEvent;
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{sse_frame, stream_event_frame, validate_project_id};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssistantRunEventsQuery {
    after_seq: Option<i64>,
}

/// Observes a durable run over persisted events. The browser reconnects with
/// `after_seq` and replays what it missed; the worker owns execution, so a
/// disconnect never cancels the run.
#[utoipa::path(
    get,
    path = "/projects/{project_id}/assistant/runs/{run_id}/events",
    operation_id = "streamAssistantRunEvents",
    tag = "assistant",
    params(("project_id" = Uuid, Path), ("run_id" = Uuid, Path)),
    responses(
        (status = 200, description = "Server-sent run events", content_type = "text/event-stream", body = String),
        (status = 404, description = "Run not found", body = ErrorResponse),
    )
)]
pub(crate) async fn stream_run_events(
    State(state): State<AppState>,
    Path((project_id, run_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<AssistantRunEventsQuery>,
) -> Result<Response, ApiError> {
    validate_project_id(project_id)?;
    let record = deepref_postgres::get_assistant_agent_run(&state.pool, run_id)
        .await
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    let record = match record.filter(|record| record.project_id == project_id) {
        Some(record) => record,
        None => return Err(ApiError::NotFound("assistant run not found".to_owned())),
    };
    let terminal = record.status.terminal();
    let stream = futures::stream::unfold(
        RunEventPoll {
            state,
            project_id,
            run_id,
            after_seq: query.after_seq.unwrap_or(-1),
            terminal,
            buffer: std::collections::VecDeque::new(),
        },
        |mut poll: RunEventPoll| async move {
            loop {
                if let Some(frame) = poll.buffer.pop_front() {
                    return Some((frame, poll));
                }
                let events = deepref_postgres::list_assistant_run_events(
                    &poll.state.pool,
                    poll.run_id,
                    poll.after_seq,
                    200,
                )
                .await
                .unwrap_or_default();
                for event in &events {
                    poll.after_seq = poll.after_seq.max(event.seq);
                    if let Some(frame) =
                        run_event_frame(&poll.state.pool, poll.project_id, event).await
                    {
                        if event.kind == "done" || event.kind == "error" {
                            poll.terminal = true;
                        }
                        poll.buffer.push_back(frame);
                    }
                }
                if poll.buffer.is_empty() && poll.terminal {
                    return None;
                }
                if poll.buffer.is_empty() {
                    // Re-read terminal status on empty rounds: a client that
                    // resumes past the done/error row in the milliseconds
                    // before the status flip would otherwise long-poll until
                    // it gives up. A deleted run ends the stream as well.
                    let status =
                        deepref_postgres::get_assistant_agent_run(&poll.state.pool, poll.run_id)
                            .await
                            .ok()
                            .flatten()
                            .map(|record| record.status);
                    match status {
                        None => return None,
                        Some(status) if status.terminal() => return None,
                        _ => {}
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        },
    );
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

struct RunEventPoll {
    state: AppState,
    project_id: Uuid,
    run_id: Uuid,
    after_seq: i64,
    terminal: bool,
    buffer: std::collections::VecDeque<Result<Event, std::convert::Infallible>>,
}

/// Fetches the full plan behind a `plan` frame, retrying briefly. The worker
/// emits the `plan` event before its completion transaction commits the plan
/// row (the `done` event must exist before the run flips terminal), so a
/// first-delivery lookup can land in that millisecond window; without the
/// retry the frame resolves to Null and the observer never sees the plan.
async fn resolve_plan_frame(pool: &sqlx::PgPool, project_id: Uuid, plan_id: Uuid) -> Option<Value> {
    const ATTEMPTS: u32 = 10;
    const DELAY: std::time::Duration = std::time::Duration::from_millis(100);
    for attempt in 0..ATTEMPTS {
        if let Ok(Some(plan)) =
            deepref_postgres::get_assistant_plan(pool, project_id, plan_id).await
            && let Ok(value) = serde_json::to_value(super::plans::plan_dto(&plan))
        {
            return Some(value);
        }
        if attempt + 1 < ATTEMPTS {
            tokio::time::sleep(DELAY).await;
        }
    }
    None
}

/// Maps one persisted run event onto the assistant wire contract. The `plan`
/// row carries only the plan id; the full plan is fetched for the frame.
async fn run_event_frame(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    event: &deepref_postgres::AssistantRunEventRecord,
) -> Option<Result<Event, std::convert::Infallible>> {
    let payload = &event.payload;
    let frame = match event.kind.as_str() {
        "status" => stream_event_frame(&AssistantStreamEvent::Status {
            message: payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        "text" => stream_event_frame(&AssistantStreamEvent::Token {
            delta: payload
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        "replace" => stream_event_frame(&AssistantStreamEvent::Replace {
            text: payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        "tool_start" => stream_event_frame(&AssistantStreamEvent::ToolStart {
            tool: payload
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            tool_call_id: payload
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            args: payload.get("args").cloned().unwrap_or(Value::Null),
        }),
        "tool_complete" => stream_event_frame(&AssistantStreamEvent::ToolComplete {
            tool: payload
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            tool_call_id: payload
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            output: payload.get("output").cloned().unwrap_or(Value::Null),
        }),
        "plan" => {
            let plan_id = payload
                .get("plan_id")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok());
            let mut plan_value = None;
            if let Some(plan_id) = plan_id {
                plan_value = resolve_plan_frame(pool, project_id, plan_id).await;
            }
            stream_event_frame(&AssistantStreamEvent::PlanProposed {
                plan: plan_value.unwrap_or(Value::Null),
            })
        }
        "done" => stream_event_frame(&AssistantStreamEvent::Done {
            message_id: payload
                .get("message_id")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                .unwrap_or_default(),
            input_tokens: payload
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: payload
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        }),
        "error" => {
            let code = payload
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let message = payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("assistant turn failed");
            sse_frame("error", &json!({ "code": code, "message": message }))
        }
        _ => return None,
    };
    // Stamp the persisted seq as the SSE id so observers resume with the
    // server cursor (`after_seq`) instead of counting frames.
    Some(frame.map(|frame| frame.id(event.seq.to_string())))
}

#[cfg(test)]
mod tests {
    use super::super::stream_event_parts;
    use super::*;
    use deepref_ai::AssistantStreamEvent;

    #[test]
    fn stream_event_frames_use_the_wire_contract() {
        let frames = [
            stream_event_parts(&AssistantStreamEvent::Token {
                delta: "hello ".to_owned(),
            }),
            stream_event_parts(&AssistantStreamEvent::ToolStart {
                tool: "get_report".to_owned(),
                tool_call_id: "call-1".to_owned(),
                args: json!({ "project_id": Uuid::nil() }),
            }),
            stream_event_parts(&AssistantStreamEvent::ToolComplete {
                tool: "get_report".to_owned(),
                tool_call_id: "call-1".to_owned(),
                output: json!({ "title": "Report" }),
            }),
            stream_event_parts(&AssistantStreamEvent::ProposalCreated {
                tool: "propose_screening_decision".to_owned(),
                review_run_id: Uuid::nil(),
                status_path: "/projects/x/review-runs/y".to_owned(),
            }),
            stream_event_parts(&AssistantStreamEvent::Done {
                message_id: Uuid::nil(),
                input_tokens: 10,
                output_tokens: 20,
            }),
        ];

        assert_eq!(frames[0].0, "token");
        assert_eq!(frames[0].1, json!({ "delta": "hello " }));
        assert_eq!(frames[1].0, "tool_start");
        assert_eq!(frames[1].1["tool"], "get_report");
        assert_eq!(frames[1].1["tool_call_id"], "call-1");
        assert_eq!(frames[2].0, "tool_complete");
        assert_eq!(frames[2].1["output"]["title"], "Report");
        assert_eq!(frames[3].0, "proposal_created");
        assert_eq!(frames[3].1["status_path"], "/projects/x/review-runs/y");
        assert_eq!(frames[4].0, "done");
        assert_eq!(frames[4].1["input_tokens"], 10);
        assert_eq!(frames[4].1["output_tokens"], 20);
    }
}
