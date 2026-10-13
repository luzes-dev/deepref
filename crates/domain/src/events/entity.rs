use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Project,
    ProjectMembership,
    Citation,
    UnresolvedReference,
    #[default]
    Work,
    Metric,
    Projection,
    DeadLetter,
}

impl EntityType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::ProjectMembership => "project_membership",
            Self::Citation => "citation",
            Self::UnresolvedReference => "unresolved_reference",
            Self::Work => "work",
            Self::Metric => "metric",
            Self::Projection => "projection",
            Self::DeadLetter => "dead_letter",
        }
    }
}
