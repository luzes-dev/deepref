use deepref_core::ArticleSummary;

#[derive(Debug, Clone, Copy)]
pub struct RankScoreInput {
    pub total_citations: i32,
    pub max_total_citations: i32,
    pub internal_citations: i32,
    pub max_internal_citations: i32,
    pub outbound_internal_references: i32,
    pub max_outbound_internal_references: i32,
    pub issued_year: Option<i32>,
    pub current_year: i32,
}

pub fn rank_score(input: RankScoreInput) -> f64 {
    let RankScoreInput {
        total_citations,
        max_total_citations,
        internal_citations,
        max_internal_citations,
        outbound_internal_references,
        max_outbound_internal_references,
        issued_year,
        current_year,
    } = input;
    let total = normalize_log(total_citations, max_total_citations);
    let internal = normalize_linear(internal_citations, max_internal_citations);
    let outbound = normalize_linear(
        outbound_internal_references,
        max_outbound_internal_references,
    );
    let recency = issued_year
        .map(|year| {
            let age = (current_year - year).max(0) as f64;
            (1.0 / (1.0 + age / 10.0)).clamp(0.0, 1.0)
        })
        .unwrap_or(0.0);

    0.45 * total + 0.40 * internal + 0.10 * outbound + 0.05 * recency
}

pub fn rerank_articles(
    mut articles: Vec<ArticleSummary>,
    current_year: i32,
) -> Vec<ArticleSummary> {
    let max_total = articles
        .iter()
        .map(|a| a.total_citations)
        .max()
        .unwrap_or(0);
    let max_internal = articles
        .iter()
        .map(|a| a.internal_citations)
        .max()
        .unwrap_or(0);
    let max_outbound = articles
        .iter()
        .map(|a| a.outbound_internal_references)
        .max()
        .unwrap_or(0);

    for article in &mut articles {
        article.rank_score = rank_score(RankScoreInput {
            total_citations: article.total_citations,
            max_total_citations: max_total,
            internal_citations: article.internal_citations,
            max_internal_citations: max_internal,
            outbound_internal_references: article.outbound_internal_references,
            max_outbound_internal_references: max_outbound,
            issued_year: article.issued_year,
            current_year,
        });
    }

    articles.sort_by(|a, b| {
        b.rank_score
            .partial_cmp(&a.rank_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    articles
}

fn normalize_log(value: i32, max_value: i32) -> f64 {
    if max_value <= 0 {
        return 0.0;
    }
    ((value.max(0) as f64) + 1.0).log10() / ((max_value as f64) + 1.0).log10()
}

fn normalize_linear(value: i32, max_value: i32) -> f64 {
    if max_value <= 0 {
        return 0.0;
    }
    (value.max(0) as f64 / max_value as f64).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_stays_between_zero_and_one() {
        let score = rank_score(RankScoreInput {
            total_citations: 100,
            max_total_citations: 100,
            internal_citations: 10,
            max_internal_citations: 10,
            outbound_internal_references: 5,
            max_outbound_internal_references: 5,
            issued_year: Some(2025),
            current_year: 2026,
        });
        assert!((0.0..=1.0).contains(&score));
        assert!(score > 0.9);
    }
}
