//! Account-notice delivery boundary. No provider SDK, credentials or fake sent
//! flag is part of this port. Implementations acknowledge actual SMTP acceptance.
use chrono::{DateTime, Utc};
use std::{future::Future, pin::Pin};
use thiserror::Error;
use uuid::Uuid;

/// Deliberately has no Debug/Serialize: the verified recipient is transient PII.
pub struct InactivityNotice {
    pub id: Uuid,
    pub recipient: String,
    pub last_human_activity: DateTime<Utc>,
    pub delete_not_before: DateTime<Utc>,
}
#[derive(Debug, Clone, Copy, Error)]
pub enum NoticeDeliveryError {
    #[error("account notice delivery is unavailable")]
    Unavailable,
    #[error("account notice delivery is not configured")]
    NotConfigured,
    #[error("account notice delivery was refused")]
    Refused,
}
impl NoticeDeliveryError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "delivery_unavailable",
            Self::NotConfigured => "delivery_not_configured",
            Self::Refused => "delivery_refused",
        }
    }
}
pub type NoticeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), NoticeDeliveryError>> + Send + 'a>>;
pub trait AccountNoticeDelivery: Send + Sync {
    /// Ok follows configured SMTP acceptance; a timeout is unresolved, never
    /// evidence of delivery. Retries preserve the notice's Message-ID.
    /// This is transport acceptance only: the caller must still persist its
    /// exact live attempt/lease and recheck activity/contact/billing authority.
    fn deliver<'a>(&'a self, notice: &'a InactivityNotice) -> NoticeFuture<'a>;
}
