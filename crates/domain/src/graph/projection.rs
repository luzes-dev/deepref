use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Lifecycle of the PostgreSQL graph projection; persisted spellings are stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionState {
    Pending,
    Ready,
    Lagging,
    Rebuilding,
    Failed,
}

impl ProjectionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Lagging => "lagging",
            Self::Rebuilding => "rebuilding",
            Self::Failed => "failed",
        }
    }

    pub const fn is_fresh(self) -> bool {
        matches!(self, Self::Ready)
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
            "ready" | "fresh" => Ok(Self::Ready),
            "lagging" => Ok(Self::Lagging),
            "rebuilding" => Ok(Self::Rebuilding),
            "failed" => Ok(Self::Failed),
            _ => Err(InvalidProjectionState(value.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectionState;

    #[test]
    fn projection_states_round_trip_and_reject_unknown_values() {
        for state in [
            ProjectionState::Pending,
            ProjectionState::Ready,
            ProjectionState::Lagging,
            ProjectionState::Rebuilding,
            ProjectionState::Failed,
        ] {
            assert_eq!(state.as_str().parse::<ProjectionState>().unwrap(), state);
        }
        assert_eq!(
            "fresh".parse::<ProjectionState>().unwrap(),
            ProjectionState::Ready
        );
        for invalid in ["", "READY", "unknown", " ready"] {
            assert!(invalid.parse::<ProjectionState>().is_err());
        }
    }
}
