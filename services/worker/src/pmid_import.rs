//! Worker side of PubMed ID imports. The job fetches the PMIDs a run still has queued,
//! in batches from PubMed. It saves each article it finds as a record through the same
//! path as a file import, queues the duplicate check, and settles the run with the DOI
//! runs' rules: a run that found some articles completes with problems, and only a run
//! that found none fails.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use deepref_postgres::{PmidOutcome, PmidRunCounts};
use deepref_providers::{MAX_IDS_PER_FETCH, PubmedArticle, PubmedClient};
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    delivery::DeliveryAction,
    store,
    workflows::{net, pubmed},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PmidImportPayload {
    project_id: Uuid,
    acquisition_id: Uuid,
}

/// Waits before a PubMed fetch is tried again.
fn backoff(attempt: i32) -> Duration {
    match attempt {
        ..=1 => Duration::from_secs(10),
        2 => Duration::from_secs(60),
        _ => Duration::from_secs(300),
    }
}

fn to_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Runs one PubMed ID import job: fetches every PMID the run still has queued.
pub async fn handle_job(
    pool: &PgPool,
    payload: Value,
    attempts: i32,
    max_attempts: i32,
) -> anyhow::Result<DeliveryAction> {
    let payload: PmidImportPayload = serde_json::from_value(payload)?;
    let Some(run) = deepref_postgres::load_pmid_run(pool, payload.acquisition_id).await? else {
        tracing::warn!(acquisition_id = %payload.acquisition_id, "PubMed import job names a missing run");
        return Ok(DeliveryAction::Ack);
    };
    if run.project_id != payload.project_id || !matches!(run.status.as_str(), "queued" | "running")
    {
        return Ok(DeliveryAction::Ack);
    }
    deepref_postgres::start_pmid_run(pool, run.run_id).await?;
    let settings = store::load_runtime_settings(pool).await?;
    let http = net::trusted_client().map_err(|error| anyhow::anyhow!(error.message))?;
    let client = PubmedClient::new(http, settings.crossref_mailto);
    loop {
        let batch =
            deepref_postgres::queued_pmids(pool, run.run_id, MAX_IDS_PER_FETCH as i64).await?;
        if batch.is_empty() {
            break;
        }
        pubmed::pace(pool)
            .await
            .map_err(|error| anyhow::anyhow!(error.message))?;
        match client.fetch(&batch).await {
            Ok(articles) => {
                save_found(pool, run.project_id, run.run_id, &batch, articles).await?;
            }
            Err(error) if error.is_retryable() && attempts < max_attempts => {
                tracing::warn!(%error, acquisition_id = %run.run_id, "PubMed fetch will be retried");
                return Ok(DeliveryAction::Nak(backoff(attempts)));
            }
            Err(error) => {
                let outcomes: Vec<PmidOutcome> = batch
                    .iter()
                    .map(|pmid| PmidOutcome::Failed {
                        pmid: pmid.clone(),
                        error: error.message().to_owned(),
                    })
                    .collect();
                deepref_postgres::save_pmid_outcomes(pool, run.project_id, run.run_id, &outcomes)
                    .await?;
            }
        }
    }
    settle(pool, run.project_id, run.run_id).await?;
    Ok(DeliveryAction::Ack)
}

/// Saves what PubMed returned for one batch: a new record for each article the project
/// does not hold yet, and "already in project" for the rest. A PMID that PubMed left out
/// is not found.
async fn save_found(
    pool: &PgPool,
    project_id: Uuid,
    run_id: Uuid,
    batch: &[String],
    articles: Vec<PubmedArticle>,
) -> anyhow::Result<()> {
    let returned_set: HashSet<&str> = articles
        .iter()
        .filter_map(|article| article.pmid.as_deref())
        .collect();
    let returned: Vec<String> = batch
        .iter()
        .filter(|pmid| returned_set.contains(pmid.as_str()))
        .cloned()
        .collect();
    let held = deepref_postgres::pmids_in_project(pool, project_id, &returned).await?;
    let outcomes = batch_outcomes(batch, articles, &held);
    deepref_postgres::save_pmid_outcomes(pool, project_id, run_id, &outcomes).await?;
    Ok(())
}

/// What became of each PMID of a batch: imported when PubMed returned an article the project
/// does not hold, already in project when it holds it, and not found when PubMed left it out.
/// An article for a PMID that was not asked for is ignored; the first article for a PMID wins.
fn batch_outcomes(
    batch: &[String],
    articles: Vec<PubmedArticle>,
    held: &HashSet<String>,
) -> Vec<PmidOutcome> {
    let mut by_pmid: HashMap<String, PubmedArticle> = HashMap::new();
    for article in articles {
        if let Some(pmid) = article.pmid.clone() {
            by_pmid.entry(pmid).or_insert(article);
        }
    }
    batch
        .iter()
        .map(|pmid| match by_pmid.get(pmid) {
            None => PmidOutcome::NotFound { pmid: pmid.clone() },
            Some(article) if held.contains(pmid) => PmidOutcome::AlreadyInProject {
                pmid: pmid.clone(),
                title: article.title.clone(),
            },
            Some(article) => PmidOutcome::Imported {
                pmid: pmid.clone(),
                title: article.title.clone(),
                record: article.to_raw_record(),
            },
        })
        .collect()
}

/// Settles a run that has no PMID left to fetch. Its status, counters and notification
/// commit together. A run that saved new records queues the duplicate check.
async fn settle(pool: &PgPool, project_id: Uuid, run_id: Uuid) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let counts: PmidRunCounts = deepref_postgres::pmid_run_counts(&mut tx, run_id).await?;
    let status = store::settled_run_status(counts.found(), counts.missed());
    deepref_postgres::complete_pmid_run(&mut tx, project_id, run_id, status, &counts).await?;
    let draft = store::acquisition_notification(
        status,
        project_id,
        run_id,
        to_i32(counts.imported),
        to_i32(counts.missed()),
    );
    deepref_postgres::record_notification_in_transaction(&mut tx, &draft).await?;
    tx.commit().await?;
    if counts.imported > 0
        && let Err(error) =
            deepref_postgres::enqueue_import_deduplication(pool, project_id, run_id).await
    {
        // The records are saved; they wait for a manual duplicate check instead.
        tracing::warn!(%error, %project_id, "could not queue the duplicate check for a PubMed import");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article(pmid: &str, title: &str) -> PubmedArticle {
        PubmedArticle {
            pmid: Some(pmid.to_owned()),
            title: Some(title.to_owned()),
            ..PubmedArticle::default()
        }
    }

    fn batch(pmids: &[&str]) -> Vec<String> {
        pmids.iter().map(|pmid| (*pmid).to_owned()).collect()
    }

    #[test]
    fn each_pmid_gets_one_outcome_in_the_order_it_was_given() {
        let outcomes = batch_outcomes(
            &batch(&["111", "222", "333"]),
            vec![article("111", "New"), article("222", "Held")],
            &HashSet::from(["222".to_owned()]),
        );
        let kinds: Vec<(&str, &str)> = outcomes
            .iter()
            .map(|outcome| match outcome {
                PmidOutcome::Imported { pmid, .. } => (pmid.as_str(), "imported"),
                PmidOutcome::AlreadyInProject { pmid, .. } => (pmid.as_str(), "already"),
                PmidOutcome::NotFound { pmid } => (pmid.as_str(), "not_found"),
                PmidOutcome::Failed { pmid, .. } => (pmid.as_str(), "failed"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("111", "imported"),
                ("222", "already"),
                ("333", "not_found")
            ]
        );
    }

    #[test]
    fn an_imported_outcome_carries_the_article_as_a_record() {
        let outcomes = batch_outcomes(
            &batch(&["111"]),
            vec![article("111", "New")],
            &HashSet::new(),
        );
        match &outcomes[0] {
            PmidOutcome::Imported { title, record, .. } => {
                assert_eq!(title.as_deref(), Some("New"));
                assert_eq!(record.title.as_deref(), Some("New"));
                assert_eq!(record.source_identifiers.len(), 1);
            }
            other => panic!("expected an import, got {other:?}"),
        }
    }

    #[test]
    fn articles_nobody_asked_for_are_ignored_and_the_first_answer_wins() {
        let outcomes = batch_outcomes(
            &batch(&["111"]),
            vec![
                article("999", "Not requested"),
                article("111", "First"),
                article("111", "Second"),
            ],
            &HashSet::new(),
        );
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            PmidOutcome::Imported { title, .. } => assert_eq!(title.as_deref(), Some("First")),
            other => panic!("expected an import, got {other:?}"),
        }
    }

    #[test]
    fn backoff_grows_and_the_count_conversion_saturates() {
        assert_eq!(backoff(1), Duration::from_secs(10));
        assert_eq!(backoff(2), Duration::from_secs(60));
        assert_eq!(backoff(5), Duration::from_secs(300));
        assert_eq!(to_i32(i64::MAX), i32::MAX);
    }
}
