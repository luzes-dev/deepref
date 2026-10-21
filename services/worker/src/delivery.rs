use std::time::Duration;

use deepref_domain::events::{DELIVERY_BACKOFF_SECONDS, MAX_DELIVERIES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAction {
    Ack,
    Nak(Duration),
    Terminate,
}

/// Authoritative state a delivery observes when it arrives: the rows of the
/// 05a state-transition matrix in `docs/saas-program/05_WORKER_TRANSPORT_MIGRATION.md`.
/// Authoritative state decides; delivery decides nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    /// `queued`, due (`available_at <= now()`), within the attempt budget.
    QueuedEligible,
    /// `queued` with a persisted due time still in the future.
    QueuedNotYetEligible,
    /// Claimed under a live lease; the lease holder finishes the work.
    ClaimedLiveLease,
    /// Claimed under a lease past `leased_until`; store lease recovery
    /// (`recover_expired_jobs`) requeues it with a persisted backoff.
    ClaimedExpiredLease,
    /// A delivery for a generation that can never become valid.
    StaleGeneration,
    /// Terminal success.
    Completed,
    /// The owning run was cancelled before this work ran.
    Cancelled,
    /// A newer generation carries the work.
    Superseded,
    /// The delivery's fence token is no longer the accepted attempt.
    LostFence,
}

impl DeliveryState {
    /// Every matrix row, in spec order. A delivery handler must resolve every
    /// one of them through [`delivery_matrix`].
    pub const ALL: [Self; 9] = [
        Self::QueuedEligible,
        Self::QueuedNotYetEligible,
        Self::ClaimedLiveLease,
        Self::ClaimedExpiredLease,
        Self::StaleGeneration,
        Self::Completed,
        Self::Cancelled,
        Self::Superseded,
        Self::LostFence,
    ];
}

/// The action half of the matrix: what the delivery handler does to the
/// authoritative row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryStep {
    /// Claim and fence the row, then execute it. The claim/fence is the only
    /// place a business attempt is spent.
    ClaimAndExecute,
    /// No-op: the authoritative row is left exactly as it stands.
    NoOp,
}

/// The 05a state-transition matrix, encoded once: `(action, transport
/// response)` for every authoritative state a delivery can observe. The
/// transport response is [`DeliveryAction`] — ACK / NAK with delay / term.
///
/// The response column's exact meanings:
/// - `QueuedEligible` ACKs after the result commit (the handler's `Ack`).
/// - `QueuedNotYetEligible` ACKs an early 05b notification once the due time
///   is persisted; 05a has no early delivery, its claim loop picks the row up
///   when due. Never NAK while waiting for due time.
/// - `ClaimedLiveLease` ACKs as an idempotent no-op; the lease holder finishes.
/// - `ClaimedExpiredLease` ACKs after recovery commits; the claim loop
///   redelivers only once the persisted due time passes.
/// - `Completed` ACKs as an idempotent no-op.
/// - `StaleGeneration`, `Cancelled`, `Superseded` and `LostFence` terminate:
///   nobody re-publishes that work, and reconciliation covers fence loss only
///   as repair.
///
/// No row NAKs: delayed retries are business retries whose due time `fail`
/// persists in `available_at`, never transport redelivery.
pub const fn delivery_matrix(state: DeliveryState) -> (DeliveryStep, DeliveryAction) {
    match state {
        DeliveryState::QueuedEligible => (DeliveryStep::ClaimAndExecute, DeliveryAction::Ack),
        DeliveryState::QueuedNotYetEligible
        | DeliveryState::ClaimedLiveLease
        | DeliveryState::ClaimedExpiredLease
        | DeliveryState::Completed => (DeliveryStep::NoOp, DeliveryAction::Ack),
        DeliveryState::StaleGeneration
        | DeliveryState::Cancelled
        | DeliveryState::Superseded
        | DeliveryState::LostFence => (DeliveryStep::NoOp, DeliveryAction::Terminate),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Malformed,
    Retryable,
    Permanent,
}

pub fn action_for(class: FailureClass, delivery_count: u64) -> DeliveryAction {
    match class {
        FailureClass::Malformed | FailureClass::Permanent => DeliveryAction::Terminate,
        FailureClass::Retryable if delivery_count >= MAX_DELIVERIES => DeliveryAction::Terminate,
        FailureClass::Retryable => {
            let index = delivery_count.saturating_sub(1) as usize;
            let seconds = DELIVERY_BACKOFF_SECONDS[index.min(DELIVERY_BACKOFF_SECONDS.len() - 1)];
            DeliveryAction::Nak(Duration::from_secs(seconds))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_policy_is_bounded() {
        assert_eq!(
            action_for(FailureClass::Retryable, 1),
            DeliveryAction::Nak(Duration::from_secs(5))
        );
        assert_eq!(
            action_for(FailureClass::Retryable, 5),
            DeliveryAction::Terminate
        );
        assert_eq!(
            action_for(FailureClass::Malformed, 1),
            DeliveryAction::Terminate
        );
    }
}
