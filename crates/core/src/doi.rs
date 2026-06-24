use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DoiError {
    #[error("DOI is empty")]
    Empty,
    #[error("DOI must start with 10. and include a suffix")]
    InvalidShape,
}

pub fn normalize_doi(input: &str) -> Result<String, DoiError> {
    let mut value = input.trim().trim_matches('.').to_lowercase();
    for prefix in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
        "doi:",
    ] {
        if let Some(stripped) = value.strip_prefix(prefix) {
            value = stripped.to_owned();
            break;
        }
    }

    value = value.trim().trim_matches('.').to_owned();
    if value.is_empty() {
        return Err(DoiError::Empty);
    }
    if !value.starts_with("10.") || !value.contains('/') {
        return Err(DoiError::InvalidShape);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_doi_forms() {
        assert_eq!(
            normalize_doi(" https://doi.org/10.1145/1234.5678. ").unwrap(),
            "10.1145/1234.5678"
        );
        assert_eq!(normalize_doi("doi:10.1000/XYZ").unwrap(), "10.1000/xyz");
    }

    #[test]
    fn rejects_invalid_dois() {
        assert_eq!(normalize_doi("").unwrap_err(), DoiError::Empty);
        assert_eq!(
            normalize_doi("not-a-doi").unwrap_err(),
            DoiError::InvalidShape
        );
    }
}
