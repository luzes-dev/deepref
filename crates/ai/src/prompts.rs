use std::collections::BTreeMap;

use crate::{AiError, sha256_bytes};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptVersion {
    identifier: String,
    content_hash: String,
}
impl PromptVersion {
    pub fn new(identifier: impl Into<String>, content: &str) -> Result<Self, AiError> {
        let identifier = identifier.into();
        let valid_version = identifier
            .rsplit_once(".v")
            .and_then(|(_, version)| version.parse::<u32>().ok())
            .is_some_and(|version| version > 0);
        if !valid_version || content.trim().is_empty() {
            return Err(AiError::PromptRegistry(
                "prompt requires immutable .vN content".to_owned(),
            ));
        }
        Ok(Self {
            identifier,
            content_hash: sha256_bytes(content.as_bytes()),
        })
    }
    pub fn identifier(&self) -> &str {
        &self.identifier
    }
    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }
}

pub struct PromptDefinition {
    pub version: PromptVersion,
    content: String,
}
impl PromptDefinition {
    pub fn new(identifier: impl Into<String>, content: impl Into<String>) -> Result<Self, AiError> {
        let content = content.into();
        Ok(Self {
            version: PromptVersion::new(identifier, &content)?,
            content,
        })
    }
    pub fn content(&self) -> &str {
        &self.content
    }
    pub fn render(&self, variables: &BTreeMap<String, String>) -> Result<String, AiError> {
        let rendered = variables
            .iter()
            .fold(self.content.clone(), |text, (key, value)| {
                text.replace(&format!("{{{{{key}}}}}"), value)
            });
        if rendered.contains("{{") || rendered.contains("}}") {
            return Err(AiError::PromptRegistry(
                "prompt contains an unresolved variable".to_owned(),
            ));
        }
        Ok(rendered)
    }
}

#[derive(Default)]
pub struct PromptRegistry {
    definitions: BTreeMap<String, PromptDefinition>,
}
impl PromptRegistry {
    pub fn register(&mut self, definition: PromptDefinition) -> Result<(), AiError> {
        let key = definition.version.identifier().to_owned();
        if let Some(existing) = self.definitions.get(&key)
            && existing.version.content_hash() != definition.version.content_hash()
        {
            return Err(AiError::PromptRegistry(
                "prompt version content is immutable".to_owned(),
            ));
        }
        self.definitions.entry(key).or_insert(definition);
        Ok(())
    }
    pub fn get(&self, identifier: &str) -> Option<&PromptDefinition> {
        self.definitions.get(identifier)
    }
    pub fn render(
        &self,
        identifier: &str,
        variables: &BTreeMap<String, String>,
    ) -> Result<String, AiError> {
        self.get(identifier)
            .ok_or_else(|| AiError::PromptRegistry("prompt version is not registered".to_owned()))?
            .render(variables)
    }
}
