#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::time::Duration;

use deepref_worker::delivery::{DeliveryAction, FailureClass, action_for};

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
