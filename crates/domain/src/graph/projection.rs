use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Lifecycle of the PostgreSQL graph projection; persisted spellings are stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionState {
    Pending,
    Ready,
}

impl ProjectionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown persisted graph projection state: {0}")]
pub struct InvalidProjectionState(pub String);

impl std::str::FromStr for ProjectionState {
    type Err = InvalidProjectionState;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "ready" => Ok(Self::Ready),
            _ => Err(InvalidProjectionState(value.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectionState;

    #[test]
    fn projection_states_round_trip_and_reject_unknown_values() {
        for state in [ProjectionState::Pending, ProjectionState::Ready] {
            assert_eq!(state.as_str().parse::<ProjectionState>().unwrap(), state);
        }
        for invalid in ["", "READY", "failed", " ready"] {
            assert!(invalid.parse::<ProjectionState>().is_err());
        }
    }
}
