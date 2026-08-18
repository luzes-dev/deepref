use std::time::Duration;

use deepref_events::{DELIVERY_BACKOFF_SECONDS, MAX_DELIVERIES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAction {
    Ack,
    Nak(Duration),
    Terminate,
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
