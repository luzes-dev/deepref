//! DeepRef model routing into Rig completion models.
//!
//! The existing flow stays: `ModelProfile` → `ModelRouter` → `ResolvedModel`.
//! This module adds the boundary converting a `ResolvedModel` into the Rig
//! completion model the agent runtime drives. Provider credentials and base
//! URLs stay host-owned: the factory receives them explicitly, never from the
//! model.
//!
//! The `ModelContract` used by calibration is derived from the same route,
//! so routing identity and agent execution cannot drift apart.

use rig_core::{DynModel, operation::Completion, providers::openai::OpenAIConfig};

use crate::{AiError, ResolvedModel};

/// Builds the Rig completion model for one resolved DeepRef route.
pub trait AgentModelFactory: Send + Sync {
    fn model(&self, route: &ResolvedModel) -> Result<DynModel<Completion>, AiError>;
}

/// OpenAI-compatible provider (opencode-go, zai) via Rig's OpenAI wire.
///
/// The base URL selects the provider deployment; the model id comes from the
/// route. Provider-specific wire quirks (reasoning blocks, session headers,
/// balance errors) stay in DeepRef's hand-rolled gateway; this factory is
/// the agent-runtime path, validated by cassette fixtures before any
/// production use.
#[derive(Debug, Clone)]
pub struct OpenAiCompatModelFactory {
    base_url: String,
    api_key: String,
}

impl OpenAiCompatModelFactory {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
        }
    }
}

impl AgentModelFactory for OpenAiCompatModelFactory {
    fn model(&self, route: &ResolvedModel) -> Result<DynModel<Completion>, AiError> {
        route.validate()?;
        if self.base_url.trim().is_empty() || self.api_key.trim().is_empty() {
            return Err(AiError::Route(
                "Rig model factory has no provider endpoint".to_owned(),
            ));
        }
        let client = OpenAIConfig::new(self.api_key.clone())
            .with_base_url(self.base_url.clone())
            .client();
        Ok(client.completion(route.model.clone()).erase())
    }
}

/// Fixed model for parity tests: every caller receives the same scripted
/// Rig model, so the adapter, hooks and event mapping are exercised without
/// a provider.
#[derive(Clone)]
pub struct StaticModelFactory {
    model: DynModel<Completion>,
}

impl StaticModelFactory {
    pub fn new(model: impl Into<DynModel<Completion>>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

impl AgentModelFactory for StaticModelFactory {
    fn model(&self, _route: &ResolvedModel) -> Result<DynModel<Completion>, AiError> {
        Ok(self.model.clone())
    }
}
