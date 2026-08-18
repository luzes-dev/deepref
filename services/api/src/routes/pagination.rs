use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use utoipa::{IntoParams, ToSchema};

use crate::error::ApiError;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 100;

#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct PaginationParams {
    /// Opaque cursor returned by the previous page.
    pub cursor: Option<String>,
    /// Page size from 1 through 100.
    pub limit: Option<i64>,
}

impl PaginationParams {
    pub fn limit(&self) -> Result<i64, ApiError> {
        let limit = self.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(ApiError::BadRequest(
                "limit must be between 1 and 100".into(),
            ));
        }
        Ok(limit)
    }

    pub fn decode<T: DeserializeOwned>(&self) -> Result<Option<T>, ApiError> {
        self.cursor.as_ref().map_or(Ok(None), |cursor| {
            let bytes = URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| ApiError::BadRequest("invalid pagination cursor".into()))?;
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| ApiError::BadRequest("invalid pagination cursor".into()))
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

pub fn page<T, K: Serialize>(
    mut items: Vec<T>,
    limit: usize,
    cursor_key: impl Fn(&T) -> K,
) -> Result<PaginatedResponse<T>, ApiError> {
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        let key = items
            .last()
            .ok_or_else(|| ApiError::BadRequest("invalid empty page boundary".into()))?;
        Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor_key(key))?))
    } else {
        None
    };
    Ok(PaginatedResponse { items, next_cursor })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip_and_limit() {
        let response = page(vec![3_i32, 2, 1], 2, |value| *value).unwrap();
        let params = PaginationParams {
            cursor: response.next_cursor,
            limit: Some(2),
        };
        assert_eq!(params.decode::<i32>().unwrap(), Some(2));
        assert_eq!(response.items, vec![3, 2]);
    }
}
