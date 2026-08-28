use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Actor, ProjectId, ReportId};

macro_rules! document_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub const fn new(value: Uuid) -> Self {
                Self(value)
            }
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self::new(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.as_uuid()
            }
        }
    };
}

document_id!(DocumentId);
document_id!(DocumentBlockId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentSource {
    Upload,
    ExternalUrl,
    Resolver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    Missing,
    External,
    Uploaded,
    Retrieving,
    Available,
    Failed,
}

impl DocumentStatus {
    pub fn transition(self, next: Self) -> Result<Self, DocumentStatusTransitionError> {
        let allowed = self == next
            || matches!(
                (self, next),
                (Self::Missing, Self::External | Self::Uploaded)
                    | (Self::External, Self::Retrieving | Self::Failed)
                    | (Self::Uploaded, Self::Available | Self::Failed)
                    | (Self::Retrieving, Self::Uploaded | Self::Failed)
                    | (Self::Failed, Self::Retrieving | Self::Uploaded)
            );
        allowed
            .then_some(next)
            .ok_or(DocumentStatusTransitionError {
                current: self,
                next,
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("document status cannot transition from {current:?} to {next:?}")]
pub struct DocumentStatusTransitionError {
    pub current: DocumentStatus,
    pub next: DocumentStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrRequirement {
    NotRequired,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentContent {
    pub mime_type: String,
    pub byte_size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub id: DocumentId,
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub original_filename: Option<String>,
    pub source: DocumentSource,
    pub status: DocumentStatus,
    pub content: Option<DocumentContent>,
    pub parser_version: Option<String>,
    pub parser_error: Option<String>,
    pub ocr_requirement: OcrRequirement,
    pub created_at: String,
    pub updated_at: String,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentBlock {
    pub id: DocumentBlockId,
    pub document_id: DocumentId,
    pub parser_version: String,
    pub page_number: u32,
    pub kind: String,
    pub section_path: Vec<String>,
    pub ordinal: u32,
    pub text: String,
    pub bbox: Option<NormalizedBoundingBox>,
    pub content_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormalizedBoundingBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl NormalizedBoundingBox {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Result<Self, BoundingBoxError> {
        if ![x, y, width, height].iter().all(|value| value.is_finite())
            || x < 0.0
            || y < 0.0
            || width <= 0.0
            || height <= 0.0
            || x + width > 1.0
            || y + height > 1.0
        {
            return Err(BoundingBoxError::OutsidePage);
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum BoundingBoxError {
    #[error("document block bounding box must be finite and within the normalized page")]
    OutsidePage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_boxes_reject_coordinates_outside_the_page() {
        assert!(NormalizedBoundingBox::new(0.9, 0.0, 0.2, 0.1).is_err());
        assert!(NormalizedBoundingBox::new(0.1, 0.2, 0.3, 0.4).is_ok());
    }

    #[test]
    fn document_status_transitions_are_explicit() {
        assert_eq!(
            DocumentStatus::Retrieving.transition(DocumentStatus::Uploaded),
            Ok(DocumentStatus::Uploaded)
        );
        assert!(
            DocumentStatus::Available
                .transition(DocumentStatus::Missing)
                .is_err()
        );
    }
}
