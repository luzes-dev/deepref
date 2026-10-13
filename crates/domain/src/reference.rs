use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
    pub doi: Option<String>,
    pub key: Option<String>,
    pub raw_unstructured: Option<String>,
    pub article_title: Option<String>,
    pub author: Option<String>,
    pub year: Option<String>,
    pub volume: Option<String>,
    pub first_page: Option<String>,
}
