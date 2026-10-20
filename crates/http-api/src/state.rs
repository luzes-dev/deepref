use std::sync::Arc;
use std::time::Instant;

use deepref_ai::{AiGateway, RoutedGateway};
use deepref_app::auth_rejections::RejectionAuditor;
use deepref_app::authorization::{DenialAuditor, PostgresAuthorizationStore};
use deepref_application::authorization::{Authorizer, PolicyDecisionPoint};
use deepref_documents::DocumentStore;
use sqlx::PgPool;

use crate::auth::AuthRuntime;

/// What the API can say about the configured AI provider without exposing
/// any secret.
#[derive(Debug, Clone, Default)]
pub struct AiRuntimeInfo {
    pub configured: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub started_at: Instant,
    pub document_store: Option<DocumentStore>,
    pub ai_gateway: Arc<dyn AiGateway>,
    pub ai_info: AiRuntimeInfo,
    pub workflow: deepref_config::WorkflowConfig,
    pub smtp: deepref_config::SmtpConfig,
    pub integrations: deepref_config::IntegrationsConfig,
    pub auth: Option<AuthRuntime>,
    /// The central policy decision point every enforcement point asks.
    pub authorizer: Arc<dyn Authorizer>,
    /// Bounded audit of authorization denials.
    pub denials: Arc<DenialAuditor>,
    /// Bounded audit of authentication rejections (11 Part B).
    pub auth_rejections: Arc<RejectionAuditor>,
    /// How often an open stream re-checks that its principal may still view
    /// the project. A revoked member's stream ends within this interval.
    pub stream_recheck: std::time::Duration,
}

/// Default for [`AppState::stream_recheck`].
pub const DEFAULT_STREAM_RECHECK: std::time::Duration = std::time::Duration::from_secs(20);

impl AppState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            authorizer: Arc::new(PolicyDecisionPoint::new(PostgresAuthorizationStore::new(
                pool.clone(),
            ))),
            denials: Arc::new(DenialAuditor::new(pool.clone())),
            auth_rejections: Arc::new(RejectionAuditor::new(pool.clone())),
            stream_recheck: DEFAULT_STREAM_RECHECK,
            pool,
            started_at: Instant::now(),
            document_store: None,
            ai_gateway: Arc::new(RoutedGateway::default()),
            ai_info: AiRuntimeInfo::default(),
            workflow: deepref_config::WorkflowConfig {
                niche_nodes_enabled: false,
            },
            smtp: deepref_config::SmtpConfig::from_source(&|_| None),
            integrations: deepref_config::IntegrationsConfig::default(),
            auth: None,
        }
    }

    /// Replaces the policy decision point. Tests use it to count decisions.
    pub fn with_authorizer(mut self, authorizer: Arc<dyn Authorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Sets how often open streams re-check their principal.
    pub fn with_stream_recheck(mut self, interval: std::time::Duration) -> Self {
        self.stream_recheck = interval;
        self
    }

    /// Records an authentication rejection into the bounded `auth.rejected`
    /// audit (11 Part B). A failed write is logged: a rejection is never
    /// turned into an error by its own audit.
    pub async fn record_auth_rejection(
        &self,
        reason: deepref_app::auth_rejections::RejectionReason,
        target: Option<deepref_domain::UserId>,
    ) {
        let request_id = crate::error::correlation_id();
        let rejection = deepref_app::auth_rejections::AuthRejection {
            reason,
            target,
            request_id: request_id.as_deref(),
        };
        if let Err(error) = self.auth_rejections.record(rejection).await {
            tracing::warn!(%error, "authentication rejection could not be audited");
        }
    }

    pub fn with_workflow_config(mut self, workflow: deepref_config::WorkflowConfig) -> Self {
        self.workflow = workflow;
        self
    }

    pub fn with_document_store(mut self, document_store: DocumentStore) -> Self {
        self.document_store = Some(document_store);
        self
    }

    pub fn with_smtp(mut self, smtp: deepref_config::SmtpConfig) -> Self {
        self.smtp = smtp;
        self
    }

    /// Installs the identity runtime. It takes effect only when the router is
    /// built with `DEEPREF_AUTH_ENABLED` on.
    pub fn with_auth(mut self, auth: AuthRuntime) -> Self {
        self.auth = Some(auth);
        self
    }

    pub fn with_integrations(mut self, integrations: deepref_config::IntegrationsConfig) -> Self {
        self.integrations = integrations;
        self
    }

    pub fn with_ai_gateway<G>(mut self, gateway: G) -> Self
    where
        G: AiGateway + 'static,
    {
        self.ai_gateway = Arc::new(gateway);
        self.ai_info.configured = true;
        self
    }

    /// Installs the structured-completion provider stack. Assistant turns run
    /// durably in the worker; nothing serves chat gateways anymore.
    pub fn with_ai_provider<G>(
        mut self,
        gateway: Arc<G>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self
    where
        G: AiGateway + 'static,
    {
        self.ai_gateway = gateway;
        self.ai_info = AiRuntimeInfo {
            configured: true,
            provider: Some(provider.into()),
            model: Some(model.into()),
        };
        self
    }

    pub fn core(pool: PgPool) -> Self {
        Self::new(pool)
    }
}
