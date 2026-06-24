use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleSummary {
    pub doi: String,
    pub title: Option<String>,
    pub issued_year: Option<i32>,
    pub work_type: Option<String>,
    pub total_citations: i32,
    pub internal_citations: i32,
    pub outbound_internal_references: i32,
    pub rank_score: f64,
}
