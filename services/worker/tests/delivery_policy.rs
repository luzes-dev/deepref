#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::time::Duration;

use deepref_worker::delivery::{
    DeliveryAction, DeliveryState, DeliveryStep, FailureClass, action_for, delivery_matrix,
};

#[test]
fn fifth_retry_terminates_and_prior_retries_are_delayed() {
    assert_eq!(
        action_for(FailureClass::Retryable, 4),
        DeliveryAction::Nak(Duration::from_secs(600))
    );
    assert_eq!(
        action_for(FailureClass::Retryable, 5),
        DeliveryAction::Terminate
    );
}

#[test]
fn malformed_messages_never_loop() {
    assert_eq!(
        action_for(FailureClass::Malformed, 1),
        DeliveryAction::Terminate
    );
}

// The 05a state-transition matrix (docs/saas-program/05_WORKER_TRANSPORT_MIGRATION.md):
// one test per row asserting the action and the ACK/NAK/term response.

#[test]
fn queued_eligible_rows_are_claimed_fenced_and_executed() {
    assert_eq!(
        delivery_matrix(DeliveryState::QueuedEligible),
        (DeliveryStep::ClaimAndExecute, DeliveryAction::Ack)
    );
}

#[test]
fn rows_due_later_are_left_alone_and_their_notification_is_acked() {
    assert_eq!(
        delivery_matrix(DeliveryState::QueuedNotYetEligible),
        (DeliveryStep::NoOp, DeliveryAction::Ack)
    );
}

#[test]
fn live_leases_are_idempotent_no_ops() {
    assert_eq!(
        delivery_matrix(DeliveryState::ClaimedLiveLease),
        (DeliveryStep::NoOp, DeliveryAction::Ack)
    );
}

#[test]
fn expired_leases_are_recovered_by_the_store_and_acked() {
    assert_eq!(
        delivery_matrix(DeliveryState::ClaimedExpiredLease),
        (DeliveryStep::NoOp, DeliveryAction::Ack)
    );
}

#[test]
fn stale_generations_terminate_without_redelivery() {
    assert_eq!(
        delivery_matrix(DeliveryState::StaleGeneration),
        (DeliveryStep::NoOp, DeliveryAction::Terminate)
    );
}

#[test]
fn completed_rows_are_idempotent_no_ops() {
    assert_eq!(
        delivery_matrix(DeliveryState::Completed),
        (DeliveryStep::NoOp, DeliveryAction::Ack)
    );
}

#[test]
fn cancelled_rows_terminate_without_redelivery() {
    assert_eq!(
        delivery_matrix(DeliveryState::Cancelled),
        (DeliveryStep::NoOp, DeliveryAction::Terminate)
    );
}

#[test]
fn superseded_rows_terminate_the_superseded_generation_only() {
    assert_eq!(
        delivery_matrix(DeliveryState::Superseded),
        (DeliveryStep::NoOp, DeliveryAction::Terminate)
    );
}

#[test]
fn lost_fences_terminate_the_stale_delivery_only() {
    assert_eq!(
        delivery_matrix(DeliveryState::LostFence),
        (DeliveryStep::NoOp, DeliveryAction::Terminate)
    );
}

/// Every matrix row is encoded, exactly one action/response pair each, and no
/// row ever NAKs: business retries persist `available_at`, transport delivery
/// never does.
#[test]
fn the_matrix_covers_every_row_and_never_naks() {
    assert_eq!(DeliveryState::ALL.len(), 9);
    for state in DeliveryState::ALL {
        let (step, response) = delivery_matrix(state);
        assert!(
            !matches!(response, DeliveryAction::Nak(_)),
            "{state:?} must not answer with a transport NAK"
        );
        if state == DeliveryState::QueuedEligible {
            assert_eq!(step, DeliveryStep::ClaimAndExecute);
        } else {
            assert_eq!(step, DeliveryStep::NoOp, "{state:?} decides nothing");
        }
    }
}
