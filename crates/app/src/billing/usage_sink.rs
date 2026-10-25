use chrono::{DateTime, Duration, Utc};
use deepref_application::{
    billing::{BillingError, BillingUsageProvider, DodoIngestOutcome, DodoUsageEvent},
    usage_export::{AiUsageFact, UsageExportOutcome, UsageExportSink},
};
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

/// Dodo downstream usage export sink.
///
/// Dispatches usage facts (`AiUsageFact`) to Dodo Payments event ingestion API
/// behind the existing transactional outbox delivery pipeline.
///
/// Invariants & Epistemic Boundaries:
/// 1. Meter activation is fail-closed: if not enabled, returns `RetryableFailure` so facts
///    remain pending in outbox without data loss.
/// 2. Zero invented quantities: `commercial_units` must be explicitly configured and positive.
///    Absent or non-positive units fail permanently (quarantined). No retime/cost fallback.
/// 3. Negative / correction usage is NOT proven by verified Dodo contract: correction facts
///    fail permanently into manual reconciliation.
/// 4. Durable canonical wire freezing: full wire & digest (event_name, model, provider,
///    principal_kind, commercial_units, customer, business, mode, original timestamp with
///    fractional precision) are frozen in `billing_usage_fact_targets` BEFORE any network call.
///    Retries across changed config or drifted scope reject drift permanently.
/// 5. Single POST invariant: transactional dispatch right is claimed before any network call.
///    Subsequent retries and concurrent workers become lookup-only.
/// 6. Provider lookup 404 is NEVER proof of non-ingestion: after dispatch intent is claimed,
///    a 404 lookup fails closed into quarantine / operator review hold.
/// 7. Grace guard restricts NEW `POST /events/ingest` to (-50m past, +4m future), but does
///    NOT skip lookup: late facts look up the event first to verify if already ingested.
/// 8. Error sanitization: raw database errors and customer references are never leaked into
///    outbox error strings.
#[derive(Clone)]
pub struct DodoUsageExportSink {
    pool: PgPool,
    dodo: Arc<dyn BillingUsageProvider>,
    meter_enabled: bool,
    event_name: String,
}

#[derive(Debug, FromRow)]
struct FrozenTargetRow {
    customer_ref: String,
    account_generation: i64,
    provider_scope: String,
    account_id: Option<Uuid>,
    business_id: Option<String>,
    event_name: Option<String>,
    original_timestamp: Option<DateTime<Utc>>,
    metadata: Option<serde_json::Value>,
    wire_digest: Option<String>,
}

#[derive(Debug, FromRow)]
struct BillingAccountCheckRow {
    id: Uuid,
    customer_ref: Option<String>,
    generation: i64,
    is_detached: bool,
}

fn compute_canonical_digest(
    provider_scope: &str,
    mode: &str,
    business_id: &str,
    customer_ref: &str,
    event_name: &str,
    timestamp_str: &str,
    metadata_val: &serde_json::Value,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(provider_scope.as_bytes());
    hasher.update(b"|");
    hasher.update(mode.as_bytes());
    hasher.update(b"|");
    hasher.update(business_id.as_bytes());
    hasher.update(b"|");
    hasher.update(customer_ref.as_bytes());
    hasher.update(b"|");
    hasher.update(event_name.as_bytes());
    hasher.update(b"|");
    hasher.update(timestamp_str.as_bytes());
    hasher.update(b"|");
    let meta_str = serde_json::to_string(metadata_val).unwrap_or_default();
    hasher.update(meta_str.as_bytes());
    hasher
        .finalize()
        .as_slice()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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

        // 4. Construct canonical wire metadata and event, preserving full fractional precision
        let timestamp_str = fact
            .settled_at
            .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
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

        let metadata_val = match serde_json::to_value(&metadata) {
            Ok(v) => v,
            Err(_) => {
                return UsageExportOutcome::PermanentFailure(
                    "failed to serialize canonical event metadata".into(),
                );
            }
        };

        // 5. Resolve or freeze target customer mapping, canonical wire, and digest BEFORE any network call
        let (customer_ref, _generation) = match self
            .get_or_freeze_target(fact, &timestamp_str, &metadata_val)
            .await
        {
            Ok(target) => target,
            Err(outcome) => return outcome,
        };

        let wire_event = DodoUsageEvent {
            customer_id: customer_ref,
            event_id: fact.fact_id.to_string(),
            event_name: self.event_name.clone(),
            timestamp: timestamp_str,
            metadata: Some(metadata),
        };

        // 6. Transactional dispatch right claim
        // Atomically claim the single right to dispatch POST /events/ingest before any network call.
        let claim_res = sqlx::query(
            "UPDATE billing_usage_fact_targets
             SET dispatch_claimed_at = now(), dispatch_state = 'dispatched'
             WHERE fact_id = $1 AND dispatch_claimed_at IS NULL",
        )
        .bind(fact.fact_id)
        .execute(&self.pool)
        .await;

        let claimed = match claim_res {
            Ok(res) => res.rows_affected() == 1,
            Err(_) => {
                return UsageExportOutcome::RetryableFailure(
                    "failed to claim export dispatch right".into(),
                );
            }
        };

        // If dispatch was already claimed (by prior attempt or concurrent worker), switch to LOOKUP-ONLY
        if !claimed {
            return self.lookup_already_dispatched(&wire_event).await;
        }

        // 7. Acceptance window check (-50m to +4m).
        // If outside window, do NOT POST blindly, but MUST lookup first in case it was already ingested!
        let now = Utc::now();
        let age = now.signed_duration_since(fact.settled_at);
        let is_outside_acceptance_window =
            age > Duration::minutes(50) || age < -Duration::minutes(4);

        if is_outside_acceptance_window {
            match self.dodo.lookup_usage_event(&wire_event.event_id).await {
                Ok(Some(record)) => {
                    if record.matches_canonical(&wire_event, self.dodo.business_id()) {
                        let _ = sqlx::query(
                            "UPDATE billing_usage_fact_targets SET dispatch_state = 'duplicate' WHERE fact_id = $1",
                        )
                        .bind(fact.fact_id)
                        .execute(&self.pool)
                        .await;
                        return UsageExportOutcome::Duplicate;
                    } else {
                        return UsageExportOutcome::PermanentFailure(
                            "canonical wire conflict on aged event lookup".into(),
                        );
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
                Err(_) => {
                    return UsageExportOutcome::PermanentFailure(
                        "lookup error for aged fact".into(),
                    );
                }
            }
        }

        // 8. Dispatch POST /events/ingest (only executed by the exclusive dispatch claim winner)
        match self.dodo.ingest_usage_event(wire_event.clone()).await {
            Ok(DodoIngestOutcome::Accepted) => {
                let _ = sqlx::query(
                    "UPDATE billing_usage_fact_targets SET dispatch_state = 'delivered' WHERE fact_id = $1",
                )
                .bind(fact.fact_id)
                .execute(&self.pool)
                .await;
                UsageExportOutcome::Accepted
            }
            Ok(DodoIngestOutcome::Conflict) => {
                // Event ID conflict: lookup to confirm complete canonical wire match
                self.verify_existing_on_provider(&wire_event).await
            }
            Ok(DodoIngestOutcome::Ambiguous) => {
                // Ambiguous POST: lookup to check if provider ingested it
                match self.dodo.lookup_usage_event(&wire_event.event_id).await {
                    Ok(Some(record)) => {
                        if record.matches_canonical(&wire_event, self.dodo.business_id()) {
                            let _ = sqlx::query(
                                "UPDATE billing_usage_fact_targets SET dispatch_state = 'duplicate' WHERE fact_id = $1",
                            )
                            .bind(fact.fact_id)
                            .execute(&self.pool)
                            .await;
                            UsageExportOutcome::Duplicate
                        } else {
                            UsageExportOutcome::PermanentFailure(
                                "canonical wire conflict on ambiguous event".into(),
                            )
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
            Err(_) => {
                UsageExportOutcome::PermanentFailure("unexpected billing provider error".into())
            }
        }
    }

    async fn lookup_already_dispatched(&self, wire_event: &DodoUsageEvent) -> UsageExportOutcome {
        match self.dodo.lookup_usage_event(&wire_event.event_id).await {
            Ok(Some(record)) => {
                if record.matches_canonical(wire_event, self.dodo.business_id()) {
                    UsageExportOutcome::Duplicate
                } else {
                    UsageExportOutcome::PermanentFailure(
                        "canonical wire conflict on previously dispatched event".into(),
                    )
                }
            }
            Ok(None) => {
                // Invariant: 404 is never proof of non-ingestion. Once dispatch was claimed,
                // failure to find the event on provider lookup must fail closed into ambiguous hold.
                UsageExportOutcome::Ambiguous(
                    "dispatch intent previously claimed; provider lookup returned 404 (non-ingestion not proven), holding for review".into()
                )
            }
            Err(BillingError::Unavailable) => UsageExportOutcome::RetryableFailure(
                "provider unavailable during lookup of previously dispatched event".into(),
            ),
            Err(_) => UsageExportOutcome::Ambiguous("lookup failed after claimed dispatch".into()),
        }
    }

    async fn get_or_freeze_target(
        &self,
        fact: &AiUsageFact,
        timestamp_str: &str,
        metadata_val: &serde_json::Value,
    ) -> Result<(String, i64), UsageExportOutcome> {
        let provider_scope = self.dodo.usage_namespace();

        // 1. Check if already frozen
        let existing: Option<FrozenTargetRow> = sqlx::query_as(
            "SELECT customer_ref, account_generation, provider_scope, account_id, business_id, event_name, original_timestamp, metadata, wire_digest
             FROM billing_usage_fact_targets WHERE fact_id = $1",
        )
        .bind(fact.fact_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| UsageExportOutcome::RetryableFailure("failed to query fact targets".into()))?;

        if let Some(target) = existing {
            // Target immutable scope must be compared even when current scoped account shares the same customer!
            if target.provider_scope != provider_scope {
                return Err(UsageExportOutcome::PermanentFailure(
                    "dodo downstream export: provider scope drifted across retries".into(),
                ));
            }

            // Verify canonical wire fields did not drift
            if target
                .business_id
                .as_deref()
                .is_some_and(|b| b != self.dodo.business_id())
            {
                return Err(UsageExportOutcome::PermanentFailure(
                    "dodo downstream export: canonical business_id drifted across retries".into(),
                ));
            }
            if target
                .event_name
                .as_deref()
                .is_some_and(|e| e != self.event_name)
            {
                return Err(UsageExportOutcome::PermanentFailure(
                    "dodo downstream export: canonical event_name drifted across retries".into(),
                ));
            }
            if target
                .original_timestamp
                .is_some_and(|ts| ts.timestamp_micros() != fact.settled_at.timestamp_micros())
            {
                return Err(UsageExportOutcome::PermanentFailure(
                    "dodo downstream export: original timestamp drifted across retries".into(),
                ));
            }
            if target.metadata.as_ref().is_some_and(|m| m != metadata_val) {
                return Err(UsageExportOutcome::PermanentFailure(
                    "dodo downstream export: canonical metadata drifted across retries".into(),
                ));
            }

            let expected_digest = compute_canonical_digest(
                provider_scope,
                "live",
                self.dodo.business_id(),
                &target.customer_ref,
                &self.event_name,
                timestamp_str,
                metadata_val,
            );
            if target
                .wire_digest
                .as_deref()
                .is_some_and(|d| d != expected_digest)
            {
                return Err(UsageExportOutcome::PermanentFailure(
                    "dodo downstream export: canonical wire digest drifted across retries".into(),
                ));
            }

            // Verify current billing account is still compatible
            let current_account: Option<BillingAccountCheckRow> = sqlx::query_as(
                "SELECT id, customer_ref, generation, detached_at IS NOT NULL AS is_detached
                 FROM billing_accounts WHERE workspace_id = $1 AND provider_scope = $2",
            )
            .bind(fact.workspace_id)
            .bind(provider_scope)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| {
                UsageExportOutcome::RetryableFailure("failed to query billing account".into())
            })?;

            match current_account {
                Some(acc) if acc.is_detached => {
                    return Err(UsageExportOutcome::PermanentFailure(
                        "billing account for workspace is detached; refusing historical export redirect".into(),
                    ));
                }
                Some(acc) if acc.customer_ref.as_deref() != Some(&target.customer_ref) => {
                    return Err(UsageExportOutcome::PermanentFailure(
                        "historical fact target customer does not match current workspace billing account".into(),
                    ));
                }
                Some(acc) if target.account_id.is_some_and(|aid| aid != acc.id) => {
                    return Err(UsageExportOutcome::PermanentFailure(
                        "workspace billing account lifecycle identity changed; refusing historical export redirect".into(),
                    ));
                }
                Some(_) => {
                    // Stable mapping lifecycle identity: anchored to (billing_accounts.id, customer_ref).
                    // Routine 08a reconciliations advancing generation on the same mapped account
                    // are harmless and allowed.
                    return Ok((target.customer_ref, target.account_generation));
                }
                None => {
                    return Err(UsageExportOutcome::PermanentFailure(
                        "billing account for workspace missing; refusing historical export redirect".into(),
                    ));
                }
            }
        }

        // 2. Look up active billing account
        let current_account: Option<BillingAccountCheckRow> = sqlx::query_as(
            "SELECT id, customer_ref, generation, detached_at IS NOT NULL AS is_detached
             FROM billing_accounts WHERE workspace_id = $1 AND provider_scope = $2",
        )
        .bind(fact.workspace_id)
        .bind(provider_scope)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| {
            UsageExportOutcome::RetryableFailure("failed to query billing account".into())
        })?;

        let (account_id, customer_ref, account_gen) = match current_account {
            Some(acc) if acc.is_detached => {
                return Err(UsageExportOutcome::PermanentFailure(
                    "billing account for workspace is detached".into(),
                ));
            }
            Some(acc) => match acc.customer_ref {
                Some(cus_ref) => (acc.id, cus_ref, acc.generation),
                None => {
                    return Err(UsageExportOutcome::RetryableFailure(
                        "billing account exists but customer reference is not yet provisioned"
                            .into(),
                    ));
                }
            },
            None => {
                return Err(UsageExportOutcome::PermanentFailure(
                    "no billing account found for workspace in provider scope".into(),
                ));
            }
        };

        let wire_digest = compute_canonical_digest(
            provider_scope,
            "live",
            self.dodo.business_id(),
            &customer_ref,
            &self.event_name,
            timestamp_str,
            metadata_val,
        );

        // 3. Freeze durable canonical wire into billing_usage_fact_targets
        let inserted = sqlx::query(
            "INSERT INTO billing_usage_fact_targets (
                fact_id, workspace_id, provider_scope, customer_ref, account_generation,
                bound_at, account_id, mode, business_id, event_name, original_timestamp, metadata,
                wire_digest, dispatch_claimed_at, dispatch_state
             )
             VALUES ($1, $2, $3, $4, $5, now(), $6, 'live', $7, $8, $9, $10, $11, NULL, 'bound')
             ON CONFLICT (fact_id) DO NOTHING",
        )
        .bind(fact.fact_id)
        .bind(fact.workspace_id)
        .bind(provider_scope)
        .bind(&customer_ref)
        .bind(account_gen)
        .bind(account_id)
        .bind(self.dodo.business_id())
        .bind(&self.event_name)
        .bind(fact.settled_at)
        .bind(metadata_val)
        .bind(&wire_digest)
        .execute(&self.pool)
        .await
        .map_err(|_| UsageExportOutcome::RetryableFailure("failed to freeze fact target".into()))?;

        if inserted.rows_affected() == 0 {
            // Concurrently inserted; re-fetch frozen target and validate against race
            let frozen: Option<FrozenTargetRow> = sqlx::query_as(
                "SELECT customer_ref, account_generation, provider_scope, account_id, business_id, event_name, original_timestamp, metadata, wire_digest
                 FROM billing_usage_fact_targets WHERE fact_id = $1",
            )
            .bind(fact.fact_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| UsageExportOutcome::RetryableFailure("failed to refetch frozen target".into()))?;

            if let Some(target) = frozen {
                if target.provider_scope != provider_scope
                    || target.customer_ref != customer_ref
                    || target.account_id.is_some_and(|aid| aid != account_id)
                    || target
                        .event_name
                        .as_deref()
                        .is_some_and(|e| e != self.event_name)
                    || target
                        .wire_digest
                        .as_deref()
                        .is_some_and(|d| d != wire_digest)
                {
                    return Err(UsageExportOutcome::PermanentFailure(
                        "concurrent target binding conflict on fact".into(),
                    ));
                }
                return Ok((target.customer_ref, target.account_generation));
            }
        }

        Ok((customer_ref, account_gen))
    }

    async fn verify_existing_on_provider(&self, expected: &DodoUsageEvent) -> UsageExportOutcome {
        match self.dodo.lookup_usage_event(&expected.event_id).await {
            Ok(Some(record)) => {
                if record.matches_canonical(expected, self.dodo.business_id()) {
                    let _ = sqlx::query(
                        "UPDATE billing_usage_fact_targets SET dispatch_state = 'duplicate' WHERE fact_id = $1",
                    )
                    .bind(Uuid::parse_str(&expected.event_id).unwrap_or_default())
                    .execute(&self.pool)
                    .await;
                    UsageExportOutcome::Duplicate
                } else {
                    UsageExportOutcome::PermanentFailure(
                        "canonical wire conflict on event lookup".into(),
                    )
                }
            }
            Ok(None) => UsageExportOutcome::PermanentFailure(
                "provider reported conflict but event was not found on lookup".into(),
            ),
            Err(BillingError::Unavailable) => UsageExportOutcome::RetryableFailure(
                "provider unavailable during conflict verification".into(),
            ),
            Err(_) => UsageExportOutcome::PermanentFailure("conflict lookup failed".into()),
        }
    }
}

impl UsageExportSink for DodoUsageExportSink {
    fn deliver<'a>(&'a self, fact: &'a AiUsageFact) -> BoxFuture<'a, UsageExportOutcome> {
        Box::pin(self.deliver_fact(fact))
    }
}
