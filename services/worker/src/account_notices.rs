//! SMTP implementation of the account-notice port. Ordinary HTTP auth and
//! scientific requests never construct or invoke this adapter.
use deepref_application::account_lifecycle::{
    AccountNoticeDelivery, InactivityNotice, NoticeDeliveryError, NoticeFuture,
};
use deepref_config::{PublicOrigin, SmtpConfig};
use std::time::Duration;

pub struct SmtpAccountNotices {
    pub smtp: SmtpConfig,
    pub origin: PublicOrigin,
}
impl AccountNoticeDelivery for SmtpAccountNotices {
    fn deliver<'a>(&'a self, notice: &'a InactivityNotice) -> NoticeFuture<'a> {
        Box::pin(async move {
            if !self.smtp.is_configured() {
                return Err(NoticeDeliveryError::NotConfigured);
            }
            let recipients = vec![notice.recipient.clone()];
            let body = format!(
                "Your DeepRef account has no recorded human interaction since {}. It is currently enrolled only in locally confirmed free workspaces.\n\nYour inactive account is scheduled for cleanup no earlier than {}. Sign in and interact with DeepRef to keep it active. Download your data from Settings before cleanup. Shared research workspaces and scientific provenance are preserved under the former-member policy.\n\nOpen DeepRef: {}/settings\n\nThis notice concerns account inactivity; it is not evidence of a payment or subscription change.",
                notice.last_human_activity,
                notice.delete_not_before,
                self.origin.as_str()
            );
            tokio::time::timeout(
                Duration::from_secs(30),
                crate::workflows::net::send_email_with_id(
                    &recipients,
                    "Your inactive DeepRef account",
                    &body,
                    &self.smtp,
                    notice.id,
                ),
            )
            .await
            .map_err(|_| NoticeDeliveryError::Unavailable)?
            .map_err(|_| NoticeDeliveryError::Unavailable)?;
            Ok(())
        })
    }
}
