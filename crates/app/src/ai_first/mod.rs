//! Title/abstract AI-first workflow. Scientific writes remain behind cohort approval.
pub(crate) mod audit;
pub(crate) mod cohorts;
mod routing;
pub(crate) use audit::record_audit_label;
pub use audit::{draw_ai_first_audit, evaluate_ai_first_audit, finalize_ai_first_cohort};
pub use cohorts::{
    AiFirstCohort, AiFirstOverview, close_ai_first_cohort, get_ai_first_overview,
    recover_ai_first_cohort, start_ai_first_cohort,
};
pub(crate) use cohorts::{invalidate_in_transaction, ledger, lock_project};
pub(crate) use routing::admit_ai_first;
pub use routing::{route_ai_first_result, sweep_ai_first};

use thiserror::Error;

fn stored<T>(column: &str, value: &str, parse: impl FnOnce(&str) -> Option<T>) -> Result<T, sqlx::Error> {
    parse(value).ok_or_else(|| sqlx::Error::ColumnDecode {
        index: column.to_owned(),
        source: format!("invalid persisted {column}: {value}").into(),
    })
}

fn cohort_status(row: &sqlx::postgres::PgRow) -> Result<deepref_review::ai_first::AiFirstCohortStatus, sqlx::Error> {
    use sqlx::Row;
    stored("status", row.try_get("status")?, deepref_review::ai_first::AiFirstCohortStatus::parse)
}
#[derive(Debug, Error)]
pub enum AiFirstError {
    #[error("AI-first resource not found")]
    NotFound,
    #[error("AI-first request is invalid: {0}")]
    Invalid(String),
    #[error("AI-first state refused: {0}")]
    Refused(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Review(#[from] crate::ReviewPreparationError),
    #[error(transparent)]
    Screening(#[from] crate::screening::ScreeningError),
}
