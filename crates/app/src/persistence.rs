//! One classification of `sqlx::Error` for every caller.
//!
//! Callers used to compare SQLSTATE strings by hand, and everything they did
//! not recognise became an opaque failure. [`classify`] sorts an error into the
//! few outcomes a caller can act on: report "not found", report a conflict,
//! try again, or treat it as a bug. [`with_retry`] re-runs a whole
//! serializable or repeatable-read transaction when PostgreSQL rolled it back
//! because it lost a serialization race or a deadlock.

use std::{future::Future, time::Duration};

/// What a caller can do about a persistence failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistenceError {
    /// The row or object the operation needed does not exist.
    NotFound,
    /// A unique or exclusion constraint rejected a write, usually because a
    /// concurrent write got there first.
    Conflict { constraint: Option<String> },
    /// The operation may succeed if it is attempted again.
    Retryable(RetryReason),
    /// A foreign-key, not-null or check constraint rejected the write.
    Integrity {
        violation: IntegrityViolation,
        constraint: Option<String>,
    },
    /// Anything else: a bug, a schema mismatch or an unexpected driver error.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryReason {
    /// SQLSTATE 40001. PostgreSQL rolled the transaction back.
    SerializationFailure,
    /// SQLSTATE 40P01. PostgreSQL rolled the transaction back.
    Deadlock,
    /// The database is unreachable, out of connections, shutting down, or a
    /// lock wait timed out. Whether a commit in flight succeeded is unknown.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityViolation {
    /// SQLSTATE 23503, typically a reference to a row deleted concurrently.
    ForeignKey,
    /// SQLSTATE 23502.
    NotNull,
    /// SQLSTATE 23514.
    Check,
    /// Any other class-23 SQLSTATE.
    Other,
}

impl RetryReason {
    /// True when PostgreSQL guarantees the failed transaction left no trace,
    /// so re-running it from the start is always safe.
    pub fn rolled_back(self) -> bool {
        matches!(self, Self::SerializationFailure | Self::Deadlock)
    }
}

impl PersistenceError {
    pub fn is_foreign_key_violation(&self) -> bool {
        matches!(
            self,
            Self::Integrity {
                violation: IntegrityViolation::ForeignKey,
                ..
            }
        )
    }

    pub fn is_unique_conflict(&self) -> bool {
        matches!(self, Self::Conflict { .. })
    }

    /// True when the whole transaction can be re-run without side effects.
    pub fn is_transaction_retryable(&self) -> bool {
        matches!(self, Self::Retryable(reason) if reason.rolled_back())
    }
}

/// Classifies a driver error by its variant and, for database errors, by
/// SQLSTATE.
pub fn classify(error: &sqlx::Error) -> PersistenceError {
    match error {
        sqlx::Error::RowNotFound => PersistenceError::NotFound,
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) => {
            PersistenceError::Retryable(RetryReason::Unavailable)
        }
        sqlx::Error::Database(database) => {
            classify_sqlstate(database.code().as_deref(), database.constraint())
        }
        _ => PersistenceError::Other,
    }
}

fn classify_sqlstate(code: Option<&str>, constraint: Option<&str>) -> PersistenceError {
    let Some(code) = code else {
        return PersistenceError::Other;
    };
    let constraint = constraint.map(str::to_owned);
    let integrity = |violation| PersistenceError::Integrity {
        violation,
        constraint: constraint.clone(),
    };
    match code {
        // unique_violation, exclusion_violation
        "23505" | "23P01" => PersistenceError::Conflict { constraint },
        "23503" => integrity(IntegrityViolation::ForeignKey),
        "23502" => integrity(IntegrityViolation::NotNull),
        "23514" => integrity(IntegrityViolation::Check),
        _ if code.starts_with("23") => integrity(IntegrityViolation::Other),
        "40001" => PersistenceError::Retryable(RetryReason::SerializationFailure),
        "40P01" => PersistenceError::Retryable(RetryReason::Deadlock),
        // Class 08 connection exceptions, too_many_connections,
        // lock_not_available, and the class 57P shutdown/startup states.
        _ if code.starts_with("08") => PersistenceError::Retryable(RetryReason::Unavailable),
        "53300" | "55P03" | "57P01" | "57P02" | "57P03" => {
            PersistenceError::Retryable(RetryReason::Unavailable)
        }
        // no_data_found, raised by `SELECT ... INTO STRICT` and by functions
        // that report a missing row explicitly.
        "P0002" => PersistenceError::NotFound,
        _ => PersistenceError::Other,
    }
}

/// Errors that may carry a `sqlx::Error` somewhere inside them.
pub trait PersistenceFailure {
    fn persistence_error(&self) -> Option<PersistenceError>;
}

impl PersistenceFailure for sqlx::Error {
    fn persistence_error(&self) -> Option<PersistenceError> {
        Some(classify(self))
    }
}

impl PersistenceFailure for anyhow::Error {
    fn persistence_error(&self) -> Option<PersistenceError> {
        self.chain()
            .find_map(|cause| cause.downcast_ref::<sqlx::Error>())
            .map(classify)
    }
}

/// How often and how patiently [`with_retry`] re-runs a transaction.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Total attempts, including the first.
    pub max_attempts: u32,
    /// Upper bound of the randomised wait before the second attempt. It doubles
    /// for every later attempt.
    pub base_delay: Duration,
    /// Upper bound of any single wait.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_delay: Duration::from_millis(25),
            max_delay: Duration::from_secs(1),
        }
    }
}

impl RetryPolicy {
    /// "Full jitter": a uniformly random wait up to the exponential bound, so
    /// transactions that collided once do not collide again in lockstep.
    fn delay_before(&self, next_attempt: u32) -> Duration {
        let exponent = next_attempt.saturating_sub(2).min(16);
        let bound = self
            .base_delay
            .saturating_mul(1_u32 << exponent)
            .min(self.max_delay);
        let bound_nanos = u64::try_from(bound.as_nanos()).unwrap_or(u64::MAX);
        if bound_nanos == 0 {
            return Duration::ZERO;
        }
        let random = uuid::Uuid::new_v4().as_u64_pair().0;
        Duration::from_nanos(random % bound_nanos)
    }
}

/// Runs `operation`, and runs it again while it fails with a serialization
/// failure or a deadlock, up to `policy.max_attempts` times in total.
///
/// `operation` must own its whole transaction: begin, set the isolation level,
/// do the work and commit. Only errors where PostgreSQL rolled the transaction
/// back are retried; an unavailable database is not, because a commit that was
/// in flight may already have succeeded.
pub async fn with_retry<T, E, F, Fut>(policy: RetryPolicy, mut operation: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: PersistenceFailure,
{
    let mut attempt = 1;
    loop {
        match operation().await {
            Err(error)
                if attempt < policy.max_attempts
                    && error
                        .persistence_error()
                        .is_some_and(|classified| classified.is_transaction_retryable()) =>
            {
                attempt += 1;
                let delay = policy.delay_before(attempt);
                tracing::info!(
                    attempt,
                    max_attempts = policy.max_attempts,
                    delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                    "retrying a transaction that lost a serialization race"
                );
                tokio::time::sleep(delay).await;
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        borrow::Cow,
        error::Error as StdError,
        fmt,
        sync::atomic::{AtomicU32, Ordering},
    };

    use sqlx::error::{DatabaseError, ErrorKind};

    use super::*;

    /// A database error with a chosen SQLSTATE, standing in for the
    /// `PgDatabaseError` PostgreSQL would send.
    #[derive(Debug)]
    struct FakeDatabaseError {
        code: &'static str,
        constraint: Option<&'static str>,
    }

    impl fmt::Display for FakeDatabaseError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "fake database error {}", self.code)
        }
    }

    impl StdError for FakeDatabaseError {}

    impl DatabaseError for FakeDatabaseError {
        fn message(&self) -> &str {
            "fake database error"
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.code))
        }

        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }

        fn constraint(&self) -> Option<&str> {
            self.constraint
        }

        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    fn database_error(code: &'static str, constraint: Option<&'static str>) -> sqlx::Error {
        sqlx::Error::Database(Box::new(FakeDatabaseError { code, constraint }))
    }

    #[test]
    fn unique_violation_is_a_conflict_naming_its_constraint() {
        assert_eq!(
            classify(&database_error("23505", Some("workflows_project_name_key"))),
            PersistenceError::Conflict {
                constraint: Some("workflows_project_name_key".to_owned())
            }
        );
        assert!(classify(&database_error("23P01", None)).is_unique_conflict());
    }

    #[test]
    fn foreign_key_violation_is_an_integrity_failure() {
        let classified = classify(&database_error("23503", Some("fk_project")));
        assert_eq!(
            classified,
            PersistenceError::Integrity {
                violation: IntegrityViolation::ForeignKey,
                constraint: Some("fk_project".to_owned())
            }
        );
        assert!(classified.is_foreign_key_violation());
        assert_eq!(
            classify(&database_error("23502", None)),
            PersistenceError::Integrity {
                violation: IntegrityViolation::NotNull,
                constraint: None
            }
        );
        assert_eq!(
            classify(&database_error("23514", None)),
            PersistenceError::Integrity {
                violation: IntegrityViolation::Check,
                constraint: None
            }
        );
        assert!(!classify(&database_error("23514", None)).is_foreign_key_violation());
    }

    #[test]
    fn serialization_failures_and_deadlocks_are_retryable_after_rollback() {
        let serialization = classify(&database_error("40001", None));
        assert_eq!(
            serialization,
            PersistenceError::Retryable(RetryReason::SerializationFailure)
        );
        assert!(serialization.is_transaction_retryable());
        let deadlock = classify(&database_error("40P01", None));
        assert_eq!(deadlock, PersistenceError::Retryable(RetryReason::Deadlock));
        assert!(deadlock.is_transaction_retryable());
    }

    #[test]
    fn an_unavailable_database_is_retryable_but_not_rerun_in_process() {
        for error in [
            sqlx::Error::PoolTimedOut,
            sqlx::Error::PoolClosed,
            sqlx::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionRefused)),
            database_error("57P01", None),
            database_error("53300", None),
            database_error("08006", None),
            database_error("55P03", None),
        ] {
            let classified = classify(&error);
            assert_eq!(
                classified,
                PersistenceError::Retryable(RetryReason::Unavailable),
                "{error}"
            );
            assert!(!classified.is_transaction_retryable());
        }
    }

    #[test]
    fn missing_rows_are_not_found() {
        assert_eq!(
            classify(&sqlx::Error::RowNotFound),
            PersistenceError::NotFound
        );
        assert_eq!(
            classify(&database_error("P0002", None)),
            PersistenceError::NotFound
        );
    }

    #[test]
    fn everything_else_is_other() {
        assert_eq!(
            classify(&database_error("42P01", None)),
            PersistenceError::Other
        );
        assert_eq!(
            classify(&sqlx::Error::Protocol("bad frame".to_owned())),
            PersistenceError::Other
        );
    }

    #[test]
    fn anyhow_errors_are_classified_through_their_cause_chain() {
        let error = anyhow::Error::new(database_error("40001", None)).context("recompute");
        assert_eq!(
            error.persistence_error(),
            Some(PersistenceError::Retryable(
                RetryReason::SerializationFailure
            ))
        );
        assert_eq!(anyhow::anyhow!("no database").persistence_error(), None);
    }

    fn immediate() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        }
    }

    #[tokio::test]
    async fn with_retry_reruns_a_transaction_that_lost_a_serialization_race() {
        let attempts = AtomicU32::new(0);
        let result = with_retry(immediate(), || async {
            match attempts.fetch_add(1, Ordering::SeqCst) {
                0 => Err(database_error("40001", None)),
                1 => Err(database_error("40P01", None)),
                _ => Ok("committed"),
            }
        })
        .await;
        assert_eq!(result.ok(), Some("committed"));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn with_retry_gives_up_after_the_last_attempt() {
        let attempts = AtomicU32::new(0);
        let result: Result<(), sqlx::Error> = with_retry(immediate(), || async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(database_error("40001", None))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn with_retry_does_not_rerun_other_failures() {
        for code in ["23505", "57P01", "42P01"] {
            let attempts = AtomicU32::new(0);
            let result: Result<(), sqlx::Error> = with_retry(immediate(), || async {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(database_error(code, None))
            })
            .await;
            assert!(result.is_err());
            assert_eq!(attempts.load(Ordering::SeqCst), 1, "{code}");
        }
    }

    #[test]
    fn retry_delays_stay_within_the_exponential_bound() {
        let policy = RetryPolicy {
            max_attempts: 10,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
        };
        for _ in 0..100 {
            assert!(policy.delay_before(2) < Duration::from_millis(10));
            assert!(policy.delay_before(3) < Duration::from_millis(20));
            assert!(policy.delay_before(9) < Duration::from_millis(50));
        }
    }
}
