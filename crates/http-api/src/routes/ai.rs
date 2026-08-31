use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use deepref_ai::{AiError, ScreeningStage};
use deepref_postgres::{
    AiProposalDecision, AiProposalDecisionRequest, AiProposalError, AiProposalRecord,
    ReviewedAiProposalPayload,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
mod rendering;

pub(crate) use contracts::*;
pub(crate) use generation::*;
pub(crate) use handlers::*;
pub(crate) use rendering::{map_ai_error, proposal_dto};
use rendering::{map_ai_proposal_error, map_protocol_error};
