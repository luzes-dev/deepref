use deepref_core::Reference;
use sha2::{Digest, Sha256};

pub fn unresolved_reference_id(source_doi: &str, reference: &Reference) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_doi.as_bytes());
    hasher.update(reference.raw_unstructured.as_deref().unwrap_or_default());
    hasher.update(reference.article_title.as_deref().unwrap_or_default());
    let digest = hasher.finalize();
    let mut id = String::with_capacity(digest.len() * 2);
    for byte in digest {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}
