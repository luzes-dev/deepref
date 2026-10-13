use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct PdfConfig {
    pub library_path: Option<PathBuf>,
}

impl PdfConfig {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Self {
        Self { library_path: get("PDFIUM_LIBRARY_PATH").map(PathBuf::from) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_path_preserves_explicit_empty_and_whitespace_values() {
        assert!(PdfConfig::from_source(&|_| None).library_path.is_none());
        for value in ["", " /opt/pdfium/libpdfium.so "] {
            assert_eq!(
                PdfConfig::from_source(&|_| Some(value.into())).library_path,
                Some(PathBuf::from(value)),
            );
        }
    }
}
