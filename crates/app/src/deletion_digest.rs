use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use thiserror::Error;

/// An HMAC key of any length is accepted by HMAC-SHA256; this error protects
/// callers from silently omitting a digest if the algorithm rejects a key.
#[derive(Debug, Error)]
#[error("could not initialize the deletion-address HMAC")]
pub(crate) struct DeletionDigestError;

/// Returns lowercase hexadecimal HMAC-SHA256 for an already-normalized email.
pub(crate) fn hmac_normalized_email(
    normalized_email: &str,
    key: &[u8],
) -> Result<String, DeletionDigestError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| DeletionDigestError)?;
    mac.update(normalized_email.as_bytes());
    let bytes = mac.finalize().into_bytes();
    let mut output = String::with_capacity(bytes.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes.iter().copied() {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(output)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha256_matches_the_standard_vector() {
        let digest = hmac_normalized_email("Hi There", &[0x0b; 20]).unwrap();
        assert_eq!(
            digest,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }
}
