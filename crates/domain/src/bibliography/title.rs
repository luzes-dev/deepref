use unicode_categories::UnicodeCategories;
use unicode_normalization::UnicodeNormalization;

/// Normalize a bibliography title for deterministic comparison and database
/// shortlisting.
///
/// NFKC makes canonically equivalent and compatibility spellings comparable.
/// Unicode punctuation becomes a word boundary, while symbols (for example
/// `β`, `±`, and `→`) remain meaningful scientific content. The output is
/// deliberately conservative: it does not remove digits or symbols and it is
/// safe to persist as a comparison key.
pub fn normalize_bibliography_title(input: &str) -> String {
    let mut normalized = String::new();
    let mut pending_boundary = false;

    for character in input.nfkc().flat_map(char::to_lowercase) {
        if character.is_whitespace() || character.is_punctuation() || character.is_other() {
            pending_boundary = !normalized.is_empty();
            continue;
        }

        if pending_boundary {
            normalized.push(' ');
            pending_boundary = false;
        }
        normalized.push(character);
    }

    normalized.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::normalize_bibliography_title;

    #[test]
    fn normalizes_unicode_equivalence() {
        assert_eq!(
            normalize_bibliography_title("Ｆｉｂｅｒ Café"),
            normalize_bibliography_title("Fiber Cafe\u{301}")
        );
    }

    #[test]
    fn turns_punctuation_and_whitespace_into_one_boundary() {
        assert_eq!(
            normalize_bibliography_title("  A—study: of...  β  "),
            "a study of β"
        );
    }

    #[test]
    fn preserves_digits_and_scientific_symbols() {
        assert_eq!(
            normalize_bibliography_title("p53 β-catenin ± 2→3"),
            "p53 β catenin ± 2→3"
        );
    }

    proptest! {
        #[test]
        fn normalization_is_idempotent(input in any::<String>()) {
            let once = normalize_bibliography_title(&input);
            prop_assert_eq!(normalize_bibliography_title(&once), once);
        }

        #[test]
        fn punctuation_does_not_change_word_boundaries(
            left in "[A-Za-z0-9β]{0,20}",
            right in "[A-Za-z0-9β]{0,20}",
        ) {
            let punctuated = format!("{left}!!!{right}");
            let separated = format!("{left} {right}");
            prop_assert_eq!(
                normalize_bibliography_title(&punctuated),
                normalize_bibliography_title(&separated)
            );
        }
    }
}
