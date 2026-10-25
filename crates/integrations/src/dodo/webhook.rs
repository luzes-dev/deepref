use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

pub(super) fn keys(config: &DodoConfig) -> Result<Vec<Vec<u8>>, BillingError> {
    config
        .webhook_keys()
        .map(|s| {
            let key = STANDARD
                .decode(s.strip_prefix("whsec_").unwrap_or(s))
                .map_err(|_| BillingError::Contract)?;
            if key.len() < 16 || key.len() > 128 {
                return Err(BillingError::Contract);
            }
            Ok(key)
        })
        .collect()
}

pub(super) fn verify(
    config: &DodoConfig,
    keys: &[Vec<u8>],
    body: &[u8],
    headers: WebhookHeaders<'_>,
    now: chrono::DateTime<Utc>,
) -> Result<VerifiedBillingWebhook, BillingError> {
    if body.len() > MAX_RESPONSE
        || headers.id.is_empty()
        || headers.id.len() > 128
        || !headers
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || headers.timestamp.len() > 20
        || headers.signatures.len() > 2048
    {
        return Err(BillingError::Refused);
    }
    let timestamp: i64 = headers
        .timestamp
        .parse()
        .map_err(|_| BillingError::Refused)?;
    if now.timestamp().abs_diff(timestamp) > 300 {
        return Err(BillingError::Refused);
    }
    let mut valid = false;
    for signature in headers.signatures.split_ascii_whitespace().take(16) {
        let Some(encoded) = signature.strip_prefix("v1,") else {
            continue;
        };
        let Ok(signature) = STANDARD.decode(encoded) else {
            continue;
        };
        for key in keys {
            let mut mac =
                Hmac::<Sha256>::new_from_slice(key).map_err(|_| BillingError::Contract)?;
            mac.update(headers.id.as_bytes());
            mac.update(b".");
            mac.update(headers.timestamp.as_bytes());
            mac.update(b".");
            mac.update(body);
            valid |= mac.verify_slice(&signature).is_ok();
        }
    }
    if !valid {
        return Err(BillingError::Refused);
    }
    // Authentication of exact bytes precedes JSON parsing.
    let payload: Value = serde_json::from_slice(body).map_err(|_| BillingError::Contract)?;
    if payload.get("business_id").and_then(Value::as_str) != Some(config.business_id.as_str()) {
        return Err(BillingError::Refused);
    }
    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 80
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'.' || b == b'_')
        })
        .ok_or(BillingError::Contract)?
        .to_owned();
    let timestamp = payload
        .get("timestamp")
        .and_then(Value::as_str)
        .ok_or(BillingError::Contract)?;
    chrono::DateTime::parse_from_rfc3339(timestamp).map_err(|_| BillingError::Contract)?;
    let data = payload
        .get("data")
        .filter(|v| v.is_object())
        .ok_or(BillingError::Contract)?;
    let customer = customer_from_data(data)?;
    let adverse = commercial_review_required(&event_type);
    Ok(VerifiedBillingWebhook {
        identity: headers.id.into(),
        event_type,
        customer,
        payload,
        adverse,
    })
}

pub(super) fn customer_from_data(data: &Value) -> Result<Option<CustomerReference>, BillingError> {
    data.pointer("/customer/customer_id")
        .or_else(|| data.get("customer_id"))
        .and_then(Value::as_str)
        .map(|s| CustomerReference::new(s.into()))
        .transpose()
}
