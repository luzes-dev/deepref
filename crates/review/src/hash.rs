use std::{fmt, fmt::Write as _};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ReviewError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReviewHash(String);

impl ReviewHash {
    pub fn parse(value: impl Into<String>) -> Result<Self, ReviewError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ReviewError::InvalidHash(
                "expected lowercase SHA-256".to_owned(),
            ));
        }
        Ok(Self(value))
    }

    pub fn digest_bytes(value: impl AsRef<[u8]>) -> Self {
        let digest = Sha256::digest(value.as_ref());
        let mut encoded = String::with_capacity(64);
        for byte in digest {
            let _ = write!(&mut encoded, "{byte:02x}");
        }
        Self(encoded)
    }

    pub(crate) fn digest_json<T: Serialize>(value: &T) -> Result<Self, ReviewError> {
        let bytes = serde_json::to_vec(value)
            .map_err(|error| ReviewError::InvalidHash(error.to_string()))?;
        Ok(Self::digest_bytes(bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReviewHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
