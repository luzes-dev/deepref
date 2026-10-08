//! Webhook tokens and HMAC-SHA256 request signing.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

/// Identifies one notification from one workflow about the same records with
/// the same words. A run that repeats it adds nothing new. `None` when no
/// record is involved: then every run is a new event and nothing is merged.
pub fn notification_fingerprint(
    workflow_id: Uuid,
    record_ids: &[Uuid],
    title: &str,
    message: &str,
) -> Option<String> {
    if record_ids.is_empty() {
        return None;
    }
    let mut ids: Vec<String> = record_ids.iter().map(Uuid::to_string).collect();
    ids.sort();
    ids.dedup();
    let mut hasher = Sha256::new();
    hasher.update(workflow_id.to_string().as_bytes());
    hasher.update(b"\n");
    hasher.update(ids.join(",").as_bytes());
    hasher.update(b"\n");
    hasher.update(title.as_bytes());
    hasher.update(b"\n");
    hasher.update(message.as_bytes());
    Some(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

fn random_hex(uuids: usize) -> String {
    (0..uuids)
        .map(|_| Uuid::new_v4().simple().to_string())
        .collect()
}

/// A URL-safe random token (128 bits) identifying a workflow endpoint.
pub fn generate_token() -> String {
    random_hex(1)
}

/// A random signing secret (256 bits).
pub fn generate_secret() -> String {
    random_hex(2)
}

/// `sha256=<hex>` over the raw request body, the format senders put in the
/// `X-DeepRef-Signature` header.
pub fn sign_payload(secret: &str, body: &[u8]) -> String {
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return String::new();
    };
    mac.update(body);
    format!("sha256={}", hex(&mac.finalize().into_bytes()))
}

/// Constant-time verification of a `sha256=<hex>` signature header.
pub fn verify_signature(secret: &str, body: &[u8], header: &str) -> bool {
    let provided = header.trim();
    let provided = provided.strip_prefix("sha256=").unwrap_or(provided);
    let Some(provided) = unhex(provided) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&provided).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_round_trips_and_rejects_tampering() {
        let secret = generate_secret();
        let body = br#"{"hello":"world"}"#;
        let signature = sign_payload(&secret, body);
        assert!(signature.starts_with("sha256="));
        assert!(verify_signature(&secret, body, &signature));
        assert!(verify_signature(
            &secret,
            body,
            signature.trim_start_matches("sha256=")
        ));
        assert!(!verify_signature(&secret, b"{}", &signature));
        assert!(!verify_signature("other", body, &signature));
        assert!(!verify_signature(&secret, body, "sha256=zz"));
        assert!(!verify_signature(&secret, body, ""));
    }

    #[test]
    fn known_vector() {
        // RFC 4231 test case 2.
        let signature = sign_payload("Jefe", b"what do ya want for nothing?");
        assert_eq!(
            signature,
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn tokens_are_unique_and_long_enough() {
        assert_ne!(generate_token(), generate_token());
        assert_eq!(generate_token().len(), 32);
        assert_eq!(generate_secret().len(), 64);
    }
}
