use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use deepref_ai::{
    AiError, AiTaskRunner, AppraisalAnswerSchema, AppraisalPrefillDomain, AppraisalPrefillEvidence,
    AppraisalPrefillInput, AppraisalPrefillQuestion, CriterionPrompt, DedupeInput, DedupeTask,
    IdentityProvenance, ScreeningEvidence, ScreeningEvidenceField, ScreeningInput, ScreeningStage,
    ScreeningTask, ScreeningTaskConfig, StudyGroupingCandidate, StudyGroupingEvidence,
    StudyGroupingField, StudyGroupingInput, StudyGroupingTask, SystemClock, UuidProvider,
};
use deepref_application::{DedupeCandidate, FUZZY_PROPOSAL_THRESHOLD, score_candidate};
use deepref_domain::{
    CriterionStage, EligibilityCriterion, ScreeningStage as DomainScreeningStage,
};
use deepref_postgres::{
    AiProposalDecision, AiProposalDecisionRequest, AiProposalError, AiProposalRecord,
    ReviewedAiProposalPayload,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    pagination::{PaginatedResponse, PaginationParams, page},
    review::extract_actor,
};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

mod contracts;
mod generation;
mod handlers;
mod prompting;
mod rendering;
mod service;

pub(crate) use contracts::*;
pub(crate) use generation::*;
pub(crate) use handlers::*;
pub(crate) use prompting::run_task;
pub(crate) use rendering::{map_ai_error, proposal_dto};
pub(crate) use service::{
    AiReviewService, AppraisalPrefillReviewCommand, DuplicateReviewCommand, ScreeningReviewCommand,
    StudyGroupingReviewCommand,
};

use prompting::*;
use rendering::{map_ai_proposal_error, map_protocol_error};
