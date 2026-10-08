//! Visual workflow engine: versioned graph definitions, the node catalog,
//! publish-time validation and the pure helpers (schedules, conditions,
//! webhook signing, inbound e-mail adapters, run planning) the worker and the
//! HTTP layer share.
//!
//! Everything in this module is free of I/O so it can be unit tested without a
//! database. Persistence lives in `deepref-postgres::workflows`; node
//! execution lives in `deepref-worker::workflows`.

pub mod autonomy;
pub mod catalog;
pub mod conditions;
pub mod details;
pub mod engine;
pub mod gating;
pub mod inbound_email;
pub mod model;
pub mod preflight;
pub mod schedule;
pub mod security;
pub mod templates;
pub mod validate;

pub use autonomy::{
    AutonomyError, AutonomyFuture, AutonomyGate, AutonomyLevel, AutonomyTask, DefaultAutonomyGate,
    LOCKED_TASKS,
};
pub use catalog::{
    ConfigField, ConfigFieldKind, ConfigOption, NodeCategory, NodeTypeDef, PortDef, catalog,
    node_type,
};
pub use conditions::{
    Condition, ConditionMatch, ConditionOperator, ConditionRule, evaluate_condition, json_path,
    render_template, template_tokens,
};
pub use details::{Detail, DetailKind, FieldDetails, field_details, report_ids_in};
pub use engine::{
    InputSource, NodeRunSnapshot, NodeRunState, PlanOutcome, PlannedNode, child_iteration,
    input_sources, plan_run,
};
pub use gating::{
    AiVerdict, ReviewSlot, Routed, SCREENING_BATCH, SCREENING_POLL_SECS, SCREENING_WAIT_SECS,
    ScreeningWait, Verdict, judge_records, progress_note, summary_note,
};
pub use inbound_email::{
    InboundAttachment, InboundEmail, InboundEmailError, extract_identifiers, from_mailgun,
    from_postmark,
};
pub use model::{
    Endpoint, GraphEdge, GraphNode, PortType, Position, PublicationQuery, WorkflowGraph,
    WorkflowStatus, WorkflowTrigger, trigger_of_graph,
};
pub use preflight::{
    email_address_is_valid, email_is_configured, ip_is_public, recipients_problem,
    slack_webhook_problem, split_recipients, web_address_problem,
};
pub use schedule::{Frequency, Schedule, ScheduleError, Weekday, next_fire_after};
pub use security::{
    generate_secret, generate_token, notification_fingerprint, sign_payload, verify_signature,
};
pub use templates::{WorkflowTemplate, template, templates};
pub use validate::{
    GraphIssue, SECRET_PLACEHOLDER, SecretWrite, ValidationContext, ValidationReport, display_name,
    duration_seconds, extract_secrets, validate_graph,
};
