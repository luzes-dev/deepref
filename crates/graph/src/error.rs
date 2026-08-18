#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("Neo4j operation failed: {0}")]
    Neo4j(#[from] neo4rs::Error),
    #[error("Neo4j result decoding failed: {0}")]
    Decode(#[from] neo4rs::DeError),
    #[error("graph query timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("graph result is missing required field {0}")]
    MissingField(&'static str),
    #[error("unsupported graph mutation: {0}")]
    Unsupported(String),
}
