use chrono::{Duration, Utc};
use deepref_application::{
    billing::{BillingError, BillingUsageProvider, DodoIngestOutcome, DodoUsageEvent},
    usage_export::{AiUsageFact, UsageExportOutcome, UsageExportSink},
};
use futures::future::BoxFuture;
use sqlx::PgPool;
use std::{collections::BTreeMap, sync::Arc};

/// Dodo downstream usage export sink.
///
/// Dispatches usage facts (`AiUsageFact`) to Dodo Payments event ingestion API
/// behind the existing transactional outbox delivery pipeline.
///
/// Invariants & Epistemic Boundaries:
/// 1. Meter activation is fail-closed: if not enabled, returns `RetryableFailure` so facts
///    remain pending in outbox without data loss.
/// 2. Zero invented quantities: `commercial_units` must be explicitly configured and positive.
///    Absent or non-positive units fail permanently (quarantined).
/// 3. Negative / correction usage is NOT proven by verified Dodo contract: correction facts
///    fail permanently into manual reconciliation.
/// 4. Customer binding is frozen in `billing_usage_fact_targets` BEFORE any POST request.
/// 5. Grace guard restricts NEW `POST /events/ingest` to (-50m past, +4m future), but does
///    NOT skip lookup: late facts look up the event first to verify if already ingested.
/// 6. Complete canonical comparison: lookups verify business_id, customer_id, event_id,
///    event_name, timestamp, and deep metadata map.
/// 7. Ambiguous handling: 404 after ambiguous dispatch does NOT prove non-ingestion.
///    Ambiguous outcomes remain ambiguous for retry / manual operator hold.
#[derive(Clone)]
pub struct DodoUsageExportSink {
    pool: PgPool,
    dodo: Arc<dyn BillingUsageProvider>,
    meter_enabled: bool,
    event_name: String,
}

impl DodoUsageExportSink {
    pub fn new(
        pool: PgPool,
        dodo: Arc<dyn BillingUsageProvider>,
        meter_enabled: bool,
        event_name: String,
    ) -> Self {
        Self {
            pool,
            dodo,
            meter_enabled,
            event_name,
        }
    }

    async fn deliver_fact(&self, fact: &AiUsageFact) -> UsageExportOutcome {
        // 1. Fail closed if meter is disabled: facts remain safely in outbox retry queue
        if !self.meter_enabled {
            return UsageExportOutcome::RetryableFailure(
                "dodo usage export is disabled in runtime configuration".into(),
            );
        }

        // 2. Reject correction / negative usage: not proven by verified contract
        if fact.correction_of.is_some() {
            return UsageExportOutcome::PermanentFailure(
                "dodo downstream export: correction or negative usage not proven by verified contract; requires manual reconciliation".into(),
            );
        }

        // 3. Reject absent or non-positive commercial units: zero invented quantities
        let Some(units) = fact.commercial_units else {
            return UsageExportOutcome::PermanentFailure(
                "dodo downstream export: unconfigured commercial units on fact; refusing default or cost fallback".into(),
            );
        };
        if units <= 0 {
            return UsageExportOutcome::PermanentFailure(format!(
                "dodo downstream export: non-positive commercial units ({units}) cannot be ingested"
            ));
        }

        // 4. Resolve or freeze target customer mapping durable BEFORE any POST
        let (customer_ref, _generation) = match self.get_or_freeze_target(fact).await {
            Ok(target) => target,
            Err(outcome) => return outcome,
        };

        // 5. Construct canonical wire payload
        let timestamp_str = fact
            .settled_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut metadata = BTreeMap::new();
        metadata.insert("amount".into(), serde_json::Value::from(units));
        metadata.insert("model".into(), serde_json::Value::from(fact.model.clone()));
        metadata.insert(
            "provider".into(),
            serde_json::Value::from(fact.provider.clone()),
        );
        if let Some(kind) = fact.principal_kind {
            metadata.insert(
                "principal_kind".into(),
                serde_json::Value::from(kind.as_str()),
            );
        }

        let wire_event = DodoUsageEvent {
            customer_id: customer_ref.clone(),
            event_id: fact.fact_id.to_string(),
            event_name: self.event_name.clone(),
            timestamp: timestamp_str,
            metadata: Some(metadata),
        };

        // 6. Acceptance window check (-50m to +4m).
        // If outside window, do NOT POST blindly, but MUST lookup first in case it was already ingested!
        let now = Utc::now();
        let age = now.signed_duration_since(fact.settled_at);
        let is_outside_acceptance_window =
            age > Duration::minutes(50) || age < -Duration::minutes(4);

        if is_outside_acceptance_window {
            match self.dodo.lookup_usage_event(&wire_event.event_id).await {
                Ok(Some(record)) => {
                    if record.matches_canonical(&wire_event, self.dodo.business_id()) {
                        return UsageExportOutcome::Duplicate;
                    } else {
                        return UsageExportOutcome::PermanentFailure(format!(
                            "dodo event {} exists with mismatched canonical payload",
                            fact.fact_id
                        ));
                    }
                }
                Ok(None) => {
                    return UsageExportOutcome::PermanentFailure(format!(
                        "dodo downstream export: fact settled at {} is outside provider acceptance window (age: {}s) and not found on provider",
                        fact.settled_at,
                        age.num_seconds()
                    ));
                }
                Err(BillingError::Unavailable) => {
                    return UsageExportOutcome::RetryableFailure(
                        "provider unavailable during pre-lookup of aged fact".into(),
                    );
                }
                Err(e) => {
                    return UsageExportOutcome::PermanentFailure(format!(
                        "lookup error for aged fact: {e:?}"
                    ));
                }
            }
        }

        // 7. Dispatch POST /events/ingest
        match self.dodo.ingest_usage_event(wire_event.clone()).await {
            Ok(DodoIngestOutcome::Accepted) => UsageExportOutcome::Accepted,
            Ok(DodoIngestOutcome::Conflict) => {
                // Event ID conflict: lookup to confirm complete canonical wire match
                self.verify_existing_on_provider(&wire_event).await
            }
            Ok(DodoIngestOutcome::Ambiguous) => {
                // Ambiguous POST: lookup to check if provider ingested it
                match self.dodo.lookup_usage_event(&wire_event.event_id).await {
                    Ok(Some(record)) => {
                        if record.matches_canonical(&wire_event, self.dodo.business_id()) {
                            UsageExportOutcome::Duplicate
                        } else {
                            UsageExportOutcome::PermanentFailure(format!(
                                "canonical wire conflict on ambiguous event {}",
                                fact.fact_id
                            ))
                        }
                    }
                    Ok(None) => {
                        // 404 after ambiguous dispatch does NOT prove non-ingestion.
                        // Keep ambiguous for retry/quarantine review.
                        UsageExportOutcome::Ambiguous(
                            "ambiguous response; event not found on lookup, keeping ambiguous"
                                .into(),
                        )
                    }
                    Err(_) => UsageExportOutcome::Ambiguous(
                        "ambiguous response; lookup also failed".into(),
                    ),
                }
            }
            Ok(DodoIngestOutcome::Refused(reason)) => {
                UsageExportOutcome::PermanentFailure(format!("provider refused event: {reason}"))
            }
            Err(BillingError::Unavailable) => {
                UsageExportOutcome::RetryableFailure("provider unavailable".into())
            }
            Err(BillingError::Ambiguous) => {
                UsageExportOutcome::Ambiguous("provider returned ambiguous response".into())
            }
            Err(e) => {
                UsageExportOutcome::PermanentFailure(format!("unexpected billing error: {e:?}"))
            }
        }
    }

    async fn get_or_freeze_target(
        &self,
        fact: &AiUsageFact,
    ) -> Result<(String, i64), UsageExportOutcome> {
        let provider_scope = self.dodo.usage_namespace();

        // Check if already frozen
        let existing: Option<(String, i64)> = sqlx::query_as(
            "SELECT customer_ref, account_generation FROM billing_usage_fact_targets WHERE fact_id = $1",
        )
        .bind(fact.fact_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| UsageExportOutcome::RetryableFailure(format!("failed to query fact targets: {e}")))?;

        if let Some((customer_ref, account_gen)) = existing {
            // Verify current billing account is still compatible (not detached / remapped across retries)
            let current_account: Option<(Option<String>, i64, bool)> = sqlx::query_as(
                "SELECT customer_ref, generation, detached_at IS NOT NULL FROM billing_accounts WHERE workspace_id = $1 AND provider_scope = $2",
            )
            .bind(fact.workspace_id)
            .bind(provider_scope)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| UsageExportOutcome::RetryableFailure(format!("failed to query billing account: {e}")))?;

            match current_account {
                Some((Some(ref curr_ref), _, false)) if curr_ref == &customer_ref => {
                    return Ok((customer_ref, account_gen));
                }
                Some((Some(ref curr_ref), _, false)) => {
                    return Err(UsageExportOutcome::PermanentFailure(format!(
                        "historical fact target customer does not match current workspace billing account (frozen: {customer_ref}, current: {curr_ref})"
                    )));
                }
                Some((_, _, true)) => {
                    return Err(UsageExportOutcome::PermanentFailure(
                        "billing account for workspace is detached; refusing historical export redirect".into(),
                    ));
                }
                _ => {
                    return Err(UsageExportOutcome::PermanentFailure(
                        "billing account for workspace missing; refusing historical export redirect".into(),
                    ));
                }
            }
        }

        // Look up active billing account
        let current_account: Option<(Option<String>, i64, bool)> = sqlx::query_as(
            "SELECT customer_ref, generation, detached_at IS NOT NULL FROM billing_accounts WHERE workspace_id = $1 AND provider_scope = $2",
        )
        .bind(fact.workspace_id)
        .bind(provider_scope)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| UsageExportOutcome::RetryableFailure(format!("failed to query billing account: {e}")))?;

        let (customer_ref, account_gen) = match current_account {
            Some((Some(customer_ref), account_gen, false)) => (customer_ref, account_gen),
            Some((None, _, false)) => {
                return Err(UsageExportOutcome::RetryableFailure(
                    "billing account exists but customer_ref is not yet provisioned".into(),
                ));
            }
            Some((_, _, true)) => {
                return Err(UsageExportOutcome::PermanentFailure(
                    "billing account for workspace is detached".into(),
                ));
            }
            None => {
                return Err(UsageExportOutcome::PermanentFailure(
                    "no billing account found for workspace in provider scope".into(),
                ));
            }
        };

        // Freeze mapping into billing_usage_fact_targets
        let inserted = sqlx::query(
            "INSERT INTO billing_usage_fact_targets (fact_id, workspace_id, provider_scope, customer_ref, account_generation)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (fact_id) DO NOTHING",
        )
        .bind(fact.fact_id)
        .bind(fact.workspace_id)
        .bind(provider_scope)
        .bind(&customer_ref)
        .bind(account_gen)
        .execute(&self.pool)
        .await
        .map_err(|e| UsageExportOutcome::RetryableFailure(format!("failed to freeze fact target: {e}")))?;

        if inserted.rows_affected() == 0 {
            // Concurrently inserted; re-fetch frozen target
            let frozen: Option<(String, i64)> = sqlx::query_as(
                "SELECT customer_ref, account_generation FROM billing_usage_fact_targets WHERE fact_id = $1",
            )
            .bind(fact.fact_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| UsageExportOutcome::RetryableFailure(format!("failed to refetch frozen target: {e}")))?;

            if let Some(target) = frozen {
                return Ok(target);
            }
        }

        Ok((customer_ref, account_gen))
    }

    async fn verify_existing_on_provider(&self, expected: &DodoUsageEvent) -> UsageExportOutcome {
        match self.dodo.lookup_usage_event(&expected.event_id).await {
            Ok(Some(record)) => {
                if record.matches_canonical(expected, self.dodo.business_id()) {
                    UsageExportOutcome::Duplicate
                } else {
                    UsageExportOutcome::PermanentFailure(format!(
                        "canonical wire conflict on event {}",
                        expected.event_id
                    ))
                }
            }
            Ok(None) => UsageExportOutcome::PermanentFailure(format!(
                "provider reported conflict for event {} but event was not found on lookup",
                expected.event_id
            )),
            Err(BillingError::Unavailable) => UsageExportOutcome::RetryableFailure(
                "provider unavailable during conflict verification".into(),
            ),
            Err(e) => UsageExportOutcome::PermanentFailure(format!("conflict lookup error: {e:?}")),
        }
    }
}

impl UsageExportSink for DodoUsageExportSink {
    fn deliver<'a>(&'a self, fact: &'a AiUsageFact) -> BoxFuture<'a, UsageExportOutcome> {
        Box::pin(self.deliver_fact(fact))
    }
}
