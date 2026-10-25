//! Dodo REST contract: official TypeScript SDK revision
//! 2cff19eb469c323b583f1628e516f910c4b44c7f. See docs/dodo-billing.md.
mod webhook;
use chrono::Utc;
use deepref_application::billing::*;
use deepref_config::DodoConfig;
use deepref_domain::billing::{Entitlements, SubscriptionState};
use reqwest::{Client, Method};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::time::Duration;
use url::Url;
use uuid::Uuid;

const MAX_RESPONSE: usize = 1024 * 1024;
const MAX_PAGES: usize = 10;

pub struct DodoBilling {
    config: DodoConfig,
    client: Client,
    origin: String,
    namespace: String,
    keys: Vec<Vec<u8>>,
}

impl DodoBilling {
    pub fn new(config: DodoConfig) -> Result<Self, BillingError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| BillingError::Contract)?;
        let keys = webhook::keys(&config)?;
        Ok(Self {
            origin: config.mode.api_origin().into(),
            namespace: format!(
                "dodo:{}",
                Uuid::new_v5(
                    &Uuid::NAMESPACE_URL,
                    format!("{}:{}", config.mode.as_str(), config.business_id).as_bytes()
                )
            ),
            config,
            client,
            keys,
        })
    }

    pub fn with_base_url(config: DodoConfig, origin: String) -> Result<Self, BillingError> {
        let mut dodo = Self::new(config)?;
        dodo.origin = origin;
        Ok(dodo)
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        payload: Option<Value>,
    ) -> Result<T, BillingError> {
        let post = method == Method::POST;
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.origin))
            .bearer_auth(self.config.token());
        if let Some(payload) = payload {
            req = req.json(&payload);
        }
        // No automatic POST retries: no documented endpoint idempotency guarantee.
        let mut response = req.send().await.map_err(|_| {
            if post {
                BillingError::Ambiguous
            } else {
                BillingError::Unavailable
            }
        })?;
        if !response.status().is_success() {
            return Err(if response.status().is_server_error() {
                if post {
                    BillingError::Ambiguous
                } else {
                    BillingError::Unavailable
                }
            } else if response.status().as_u16() == 429 {
                BillingError::Unavailable
            } else {
                BillingError::Refused
            });
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE as u64)
        {
            return Err(if post {
                BillingError::Ambiguous
            } else {
                BillingError::Contract
            });
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            if post {
                BillingError::Ambiguous
            } else {
                BillingError::Unavailable
            }
        })? {
            if bytes.len() + chunk.len() > MAX_RESPONSE {
                return Err(if post {
                    BillingError::Ambiguous
                } else {
                    BillingError::Contract
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            if post {
                BillingError::Ambiguous
            } else {
                BillingError::Contract
            }
        })
    }

    pub fn business_id(&self) -> &str {
        &self.config.business_id
    }

    pub async fn ingest_usage_event(
        &self,
        event: DodoUsageEvent,
    ) -> Result<DodoIngestOutcome, BillingError> {
        self.ingest_usage_event_internal(event).await
    }

    pub async fn lookup_usage_event(
        &self,
        event_id: &str,
    ) -> Result<Option<DodoEventRecord>, BillingError> {
        self.lookup_usage_event_internal(event_id).await
    }

    async fn ingest_usage_event_internal(
        &self,
        event: DodoUsageEvent,
    ) -> Result<DodoIngestOutcome, BillingError> {
        let path = "/events/ingest";
        let req = self
            .client
            .request(Method::POST, format!("{}{path}", self.origin))
            .bearer_auth(self.config.token())
            .json(&serde_json::json!({
                "events": [event]
            }));
        let response = match req.send().await {
            Ok(res) => res,
            Err(_) => return Ok(DodoIngestOutcome::Ambiguous),
        };
        let status = response.status();
        if status.is_success() {
            let bytes = match response.bytes().await {
                Ok(b) => b,
                Err(_) => return Ok(DodoIngestOutcome::Ambiguous),
            };
            #[derive(Deserialize)]
            struct IngestResponse {
                ingested_count: usize,
            }
            let ingest_res: IngestResponse = match serde_json::from_slice(&bytes) {
                Ok(r) => r,
                Err(_) => return Ok(DodoIngestOutcome::Ambiguous),
            };
            if ingest_res.ingested_count >= 1 {
                return Ok(DodoIngestOutcome::Accepted);
            }
            return Ok(DodoIngestOutcome::Ambiguous);
        }
        if status.as_u16() == 409 {
            return Ok(DodoIngestOutcome::Conflict);
        }
        if status.is_server_error() || status.as_u16() == 429 {
            return Ok(DodoIngestOutcome::Ambiguous);
        }
        let bytes = response.bytes().await.unwrap_or_default();
        let reason = String::from_utf8_lossy(&bytes).to_string();
        Ok(DodoIngestOutcome::Refused(reason))
    }

    async fn lookup_usage_event_internal(
        &self,
        event_id: &str,
    ) -> Result<Option<DodoEventRecord>, BillingError> {
        let path = format!("/events/{event_id}");
        let req = self
            .client
            .request(Method::GET, format!("{}{path}", self.origin))
            .bearer_auth(self.config.token());
        let response = req.send().await.map_err(|_| BillingError::Unavailable)?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(
                if response.status().is_server_error() || response.status().as_u16() == 429 {
                    BillingError::Unavailable
                } else {
                    BillingError::Refused
                },
            );
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| BillingError::Unavailable)?;
        let record = serde_json::from_slice(&bytes).map_err(|_| BillingError::Contract)?;
        Ok(Some(record))
    }

    fn return_url(&self, path: &str) -> Result<String, BillingError> {
        // Closed app-owned path; queries/fragments/percent escapes cannot smuggle redirects.
        if !matches!(path, "/settings/billing" | "/settings") {
            return Err(BillingError::Refused);
        }
        Ok(format!("{}{path}", self.config.public_origin.as_str()))
    }

    fn hosted(&self, value: Option<String>, portal: bool) -> Result<HostedLink, BillingError> {
        let value = value.ok_or(BillingError::Contract)?;
        let url = Url::parse(&value).map_err(|_| BillingError::Contract)?;
        let host = if portal {
            self.config.mode.portal_host()
        } else {
            self.config.mode.checkout_host()
        };
        if value.len() > 8192
            || url.scheme() != "https"
            || url.host_str() != Some(host)
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(BillingError::Contract);
        }
        Ok(HostedLink::validated(value))
    }

    fn offering(&self, offering: Offering) -> Result<&deepref_config::DodoOffering, BillingError> {
        let key = match offering {
            Offering::Primary => "primary",
            Offering::Secondary => "secondary",
        };
        self.config
            .offerings
            .iter()
            .find(|o| o.offering == key)
            .ok_or(BillingError::OfferingUnavailable)
    }
}

#[derive(Deserialize)]
struct Customer {
    business_id: String,
    customer_id: String,
    #[serde(default)]
    metadata: std::collections::BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct Page<T> {
    items: Vec<T>,
}
#[derive(Deserialize)]
struct Checkout {
    checkout_url: Option<String>,
}
#[derive(Deserialize)]
struct Portal {
    link: String,
}
#[derive(Deserialize)]
struct Subscription {
    subscription_id: String,
    customer: LimitedCustomer,
    product_id: String,
    status: String,
}
#[derive(Deserialize)]
struct LimitedCustomer {
    customer_id: String,
}
#[derive(Deserialize)]
struct PaymentSummary {
    payment_id: String,
    customer: LimitedCustomer,
    refund_status: Option<String>,
    dispute_status: Option<String>,
}

impl BillingProvider for DodoBilling {
    fn namespace(&self) -> &str {
        &self.namespace
    }
    fn create_customer<'a>(
        &'a self,
        intent: CustomerIntent<'a>,
    ) -> BillingFuture<'a, CustomerReference> {
        Box::pin(async move {
            let customer: Customer = self.request(Method::POST, "/customers", Some(json!({ "email": intent.contact.email(), "name": intent.contact.name(), "metadata": { "deepref_operation": intent.operation.to_string() } }))).await?;
            if customer.business_id != self.config.business_id
                || customer.metadata.get("deepref_operation") != Some(&intent.operation.to_string())
            {
                return Err(BillingError::Ambiguous);
            }
            CustomerReference::new(customer.customer_id).map_err(|_| BillingError::Ambiguous)
        })
    }
    fn resolve_customer<'a>(&'a self, operation: Uuid) -> BillingFuture<'a, CustomerResolution> {
        Box::pin(async move {
            let mut found = None;
            for page in 1..=MAX_PAGES {
                let result: Page<Customer> = self
                    .request(
                        Method::GET,
                        &format!("/customers?page_number={page}&page_size=100"),
                        None,
                    )
                    .await?;
                if result.items.len() > 100 {
                    return Err(BillingError::Contract);
                }
                let end = result.items.is_empty();
                for c in result.items {
                    if c.business_id != self.config.business_id {
                        return Err(BillingError::Contract);
                    }
                    if c.metadata.get("deepref_operation") == Some(&operation.to_string()) {
                        if found.is_some() {
                            return Err(BillingError::Ambiguous);
                        }
                        found = Some(CustomerReference::new(c.customer_id)?);
                    }
                }
                if end {
                    return Ok(
                        found.map_or(CustomerResolution::Unresolved, CustomerResolution::Found)
                    );
                }
            }
            Err(BillingError::Ambiguous) // incomplete scan is never absence/uniqueness proof
        })
    }
    fn checkout<'a>(
        &'a self,
        customer: &'a CustomerReference,
        offering: Offering,
        return_path: &'a str,
    ) -> BillingFuture<'a, HostedLink> {
        Box::pin(async move {
            let return_url = self.return_url(return_path)?;
            let product = &self.offering(offering)?.product_id;
            let result: Checkout = self.request(Method::POST, "/checkouts", Some(json!({ "customer": { "customer_id": customer.expose() }, "product_cart": [{ "product_id": product, "quantity": 1 }], "return_url": return_url }))).await?;
            self.hosted(result.checkout_url, false)
        })
    }
    fn portal<'a>(
        &'a self,
        customer: &'a CustomerReference,
        return_path: &'a str,
    ) -> BillingFuture<'a, HostedLink> {
        Box::pin(async move {
            let return_url = self.return_url(return_path)?;
            // SDK places these options in query parameters, not request JSON.
            let path = format!(
                "/customers/{}/customer-portal/session?send_email=false&return_url={}",
                customer.expose(),
                urlencoding::encode(&return_url)
            );
            let result: Portal = self.request(Method::POST, &path, None).await?;
            self.hosted(Some(result.link), true)
        })
    }
    fn observe<'a>(
        &'a self,
        customer: &'a CustomerReference,
    ) -> BillingFuture<'a, SubscriptionObservation> {
        Box::pin(async move {
            // Confirm mapped customer belongs to configured business before subscription reads.
            let owner: Customer = self
                .request(
                    Method::GET,
                    &format!("/customers/{}", customer.expose()),
                    None,
                )
                .await?;
            if owner.business_id != self.config.business_id
                || owner.customer_id != customer.expose()
            {
                return Err(BillingError::Contract);
            }
            let mut subscriptions = Vec::new();
            for page in 1..=MAX_PAGES {
                let result: Page<Subscription> = self
                    .request(
                        Method::GET,
                        &format!(
                            "/subscriptions?customer_id={}&page_number={page}&page_size=100",
                            customer.expose()
                        ),
                        None,
                    )
                    .await?;
                if result.items.len() > 100 {
                    return Err(BillingError::Contract);
                }
                let end = result.items.is_empty();
                subscriptions.extend(result.items);
                if end {
                    let mut result = observation(&self.config, customer, subscriptions)?;
                    let mut evidence = Vec::new();
                    for page in 1..=MAX_PAGES {
                        let payments: Page<PaymentSummary> = self
                            .request(
                                Method::GET,
                                &format!(
                                    "/payments?customer_id={}&page_number={page}&page_size=100",
                                    customer.expose()
                                ),
                                None,
                            )
                            .await?;
                        if payments.items.len() > 100 {
                            return Err(BillingError::Contract);
                        }
                        let complete = payments.items.is_empty();
                        for payment in payments.items {
                            if payment.customer.customer_id != customer.expose() {
                                return Err(BillingError::Contract);
                            }
                            if payment.refund_status.is_some() || payment.dispute_status.is_some() {
                                evidence.push(format!(
                                    "{}:{:?}:{:?}",
                                    payment.payment_id,
                                    payment.refund_status,
                                    payment.dispute_status
                                ));
                            }
                        }
                        if complete {
                            evidence.sort();
                            if !evidence.is_empty() {
                                result.review = Some(Uuid::new_v5(
                                    &Uuid::NAMESPACE_URL,
                                    evidence.join("\n").as_bytes(),
                                ));
                            }
                            return Ok(result);
                        }
                    }
                    return Err(BillingError::Contract);
                }
            }
            Err(BillingError::Contract)
        })
    }
    fn verify(
        &self,
        body: &[u8],
        headers: WebhookHeaders<'_>,
        now: chrono::DateTime<Utc>,
    ) -> Result<VerifiedBillingWebhook, BillingError> {
        webhook::verify(&self.config, &self.keys, body, headers, now)
    }
}

fn observation(
    config: &DodoConfig,
    customer: &CustomerReference,
    subscriptions: Vec<Subscription>,
) -> Result<SubscriptionObservation, BillingError> {
    if subscriptions
        .iter()
        .any(|s| s.customer.customer_id != customer.expose())
    {
        return Err(BillingError::Contract);
    }
    // Multiple historical cancelled subscriptions are normal; multiple currently
    // active products require explicit policy, never silently union their grants.
    let active: Vec<_> = subscriptions
        .iter()
        .filter(|s| s.status == "active")
        .collect();
    let (state, entitlements) = if active.len() == 1 {
        let s = active[0];
        let e = config
            .offerings
            .iter()
            .find(|o| o.product_id == s.product_id)
            .ok_or(BillingError::OfferingUnavailable)?;
        (SubscriptionState::Active, e.entitlements.clone())
    } else if active.len() > 1 {
        return Err(BillingError::Contract);
    } else {
        // No active subscription never establishes KnownFree. We report a
        // non-granting state; event arrival order plays no role.
        let state = if subscriptions.len() == 1 {
            match subscriptions[0].status.as_str() {
                "pending" => SubscriptionState::Pending,
                "on_hold" => SubscriptionState::OnHold,
                "paused" => SubscriptionState::Paused,
                "cancelled" => SubscriptionState::Cancelled,
                "failed" => SubscriptionState::Failed,
                "expired" => SubscriptionState::Expired,
                "past_due" => SubscriptionState::PastDue,
                _ => SubscriptionState::Unknown,
            }
        } else {
            SubscriptionState::Unknown
        };
        (state, Entitlements::default())
    };
    let reference = if active.len() == 1 {
        Some(SubscriptionReference::new(
            active[0].subscription_id.clone(),
            active[0].product_id.clone(),
        )?)
    } else if subscriptions.len() == 1 {
        Some(SubscriptionReference::new(
            subscriptions[0].subscription_id.clone(),
            subscriptions[0].product_id.clone(),
        )?)
    } else {
        None
    };
    Ok(SubscriptionObservation {
        reference,
        state,
        entitlements,
        observed_at: Utc::now(),
        review: None,
    })
}

#[cfg(test)]
mod tests;

impl BillingUsageProvider for DodoBilling {
    fn usage_namespace(&self) -> &str {
        &self.namespace
    }

    fn business_id(&self) -> &str {
        &self.config.business_id
    }

    fn ingest_usage_event<'a>(
        &'a self,
        event: DodoUsageEvent,
    ) -> BillingFuture<'a, DodoIngestOutcome> {
        Box::pin(self.ingest_usage_event_internal(event))
    }

    fn lookup_usage_event<'a>(
        &'a self,
        event_id: &'a str,
    ) -> BillingFuture<'a, Option<DodoEventRecord>> {
        Box::pin(self.lookup_usage_event_internal(event_id))
    }
}
