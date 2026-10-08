//! Deterministic repair of closed-vocabulary model output.
//!
//! Every rewrite here is unambiguous: case and separator differences, aliases
//! that name the same verdict, entries that are byte-identical after
//! canonicalisation, entries for criteria the stage does not judge, and a
//! reorder into the required order when the key set is already exact. Any
//! "not applicable" style answer becomes the `unclear` value, which can never
//! create an include or exclude. Missing keys, conflicting duplicates and
//! unknown values are left as they are, so validation rejects them and the
//! runner can ask the model once more.

use serde_json::Value;

const NOT_APPLICABLE_MARKERS: &[&[&str]] = &[
    &["not", "applicable"],
    &["not", "apply"],
    &["inapplicable"],
    &["n", "a"],
    &["na"],
    &["not", "relevant"],
];

/// A keyed list of judgments, for example `criteria[].{criterion_id, judgment}`.
pub(crate) struct KeyedJudgments<'a> {
    pub(crate) array: &'a str,
    pub(crate) key: &'a str,
    pub(crate) value: &'a str,
    /// Every key must appear exactly once, in this order.
    pub(crate) expected_keys: &'a [String],
    /// Known keys that this stage does not judge. Their entries are dropped.
    pub(crate) not_judged_keys: &'a [String],
    pub(crate) allowed_values: &'a [&'a str],
    /// Exact alias to canonical value pairs. Both sides are canonical tokens.
    pub(crate) aliases: &'a [(&'a str, &'a str)],
    /// The only value used for "not applicable" style answers.
    pub(crate) unclear_value: &'a str,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct JudgmentNormalization {
    pub(crate) values_mapped: usize,
    pub(crate) duplicates_dropped: usize,
    pub(crate) not_judged_dropped: usize,
    pub(crate) reordered: bool,
}

/// Lowercase letters and digits joined by single underscores.
pub(crate) fn canonical_token(raw: &str) -> String {
    let mut token = String::with_capacity(raw.len());
    let mut separator_pending = false;
    for character in raw.trim().chars() {
        if character.is_alphanumeric() {
            if separator_pending && !token.is_empty() {
                token.push('_');
            }
            separator_pending = false;
            token.extend(character.to_lowercase());
        } else {
            separator_pending = true;
        }
    }
    token
}

/// Rewrites `root[spec.array]` in place. Keys are trimmed and lowercased,
/// values are mapped to the closed vocabulary, entries for `not_judged_keys`
/// are removed, exact duplicates are removed, and the entries are reordered
/// when their keys are exactly `expected_keys`.
pub(crate) fn normalize_keyed_judgments(
    root: &mut Value,
    spec: &KeyedJudgments<'_>,
) -> JudgmentNormalization {
    let mut report = JudgmentNormalization::default();
    let Some(slot) = root.get_mut(spec.array).and_then(Value::as_array_mut) else {
        return report;
    };
    let mut entries = std::mem::take(slot);

    for entry in &mut entries {
        let Some(object) = entry.as_object_mut() else {
            continue;
        };
        if let Some(Value::String(key)) = object.get_mut(spec.key) {
            *key = key.trim().to_lowercase();
        }
        if let Some(Value::String(value)) = object.get_mut(spec.value)
            && let Some(mapped) = canonical_value(value, spec)
            && mapped != *value
        {
            *value = mapped;
            report.values_mapped += 1;
        }
    }

    let before = entries.len();
    entries.retain(|entry| {
        !entry_key(entry, spec.key).is_some_and(|key| {
            spec.not_judged_keys
                .iter()
                .any(|not_judged| not_judged == key)
        })
    });
    report.not_judged_dropped = before - entries.len();

    let mut unique: Vec<Value> = Vec::with_capacity(entries.len());
    for entry in entries {
        if unique.contains(&entry) {
            report.duplicates_dropped += 1;
        } else {
            unique.push(entry);
        }
    }
    let mut entries = unique;

    let exact = entries.len() == spec.expected_keys.len()
        && spec.expected_keys.iter().all(|expected| {
            entries
                .iter()
                .filter(|entry| entry_key(entry, spec.key) == Some(expected.as_str()))
                .count()
                == 1
        });
    if exact {
        let mut ordered = Vec::with_capacity(entries.len());
        for expected in spec.expected_keys {
            if let Some(entry) = entries
                .iter()
                .find(|entry| entry_key(entry, spec.key) == Some(expected.as_str()))
            {
                ordered.push(entry.clone());
            }
        }
        report.reordered = ordered != entries;
        entries = ordered;
    }

    *slot = entries;
    report
}

fn entry_key<'v>(entry: &'v Value, field: &str) -> Option<&'v str> {
    entry.get(field).and_then(Value::as_str)
}

fn canonical_value(raw: &str, spec: &KeyedJudgments<'_>) -> Option<String> {
    let token = canonical_token(raw);
    if spec.allowed_values.contains(&token.as_str()) {
        return Some(token);
    }
    if let Some((_, target)) = spec.aliases.iter().find(|(alias, _)| *alias == token) {
        return Some((*target).to_owned());
    }
    if mentions_not_applicable(&token) {
        return Some(spec.unclear_value.to_owned());
    }
    single_typo_of(&token, spec.allowed_values).map(str::to_owned)
}

fn mentions_not_applicable(token: &str) -> bool {
    let parts: Vec<&str> = token.split('_').collect();
    NOT_APPLICABLE_MARKERS
        .iter()
        .any(|marker| parts.windows(marker.len()).any(|window| window == *marker))
}

/// A one-edit typo of exactly one allowed value, for example `does_not_meat`.
/// Short tokens are never corrected, and a token that is close to two values is
/// left alone.
fn single_typo_of<'a>(token: &str, allowed: &[&'a str]) -> Option<&'a str> {
    if token.chars().count() < 5 {
        return None;
    }
    let mut close = allowed
        .iter()
        .copied()
        .filter(|candidate| within_one_edit(token, candidate));
    match (close.next(), close.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

fn within_one_edit(left: &str, right: &str) -> bool {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let (longer, shorter) = if left.len() >= right.len() {
        (&left, &right)
    } else {
        (&right, &left)
    };
    if longer.len() - shorter.len() > 1 {
        return false;
    }
    let (mut i, mut j, mut edits) = (0, 0, 0_usize);
    while i < longer.len() && j < shorter.len() {
        if longer[i] == shorter[j] {
            i += 1;
            j += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return false;
        }
        i += 1;
        if longer.len() == shorter.len() {
            j += 1;
        }
    }
    edits + (longer.len() - i) <= 1
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const VALUES: &[&str] = &["meets", "does_not_meet", "unclear"];
    const ALIASES: &[(&str, &str)] = &[("met", "meets"), ("not_met", "does_not_meet")];

    fn spec<'a>(expected: &'a [String], not_judged: &'a [String]) -> KeyedJudgments<'a> {
        KeyedJudgments {
            array: "criteria",
            key: "criterion_id",
            value: "judgment",
            expected_keys: expected,
            not_judged_keys: not_judged,
            allowed_values: VALUES,
            aliases: ALIASES,
            unclear_value: "unclear",
        }
    }

    #[test]
    fn canonical_tokens_ignore_case_and_separators() {
        assert_eq!(canonical_token("  Does not MEET "), "does_not_meet");
        assert_eq!(canonical_token("does-not-meet"), "does_not_meet");
        assert_eq!(
            canonical_token("meets_not_applicable"),
            "meets_not_applicable"
        );
    }

    #[test]
    fn not_applicable_markers_map_to_unclear_and_never_to_a_verdict() {
        for raw in [
            "not_applicable",
            "Not Applicable",
            "does_not_apply",
            "meets_does_not_apply",
            "meets_not_applicable",
            "n/a",
        ] {
            assert_eq!(
                canonical_value(raw, &spec(&[], &[])).as_deref(),
                Some("unclear"),
                "{raw}"
            );
        }
        assert_eq!(
            canonical_value("meets_not_excluded_placeholder", &spec(&[], &[])),
            None
        );
        assert_eq!(
            canonical_value("does_not_meet", &spec(&[], &[])).as_deref(),
            Some("does_not_meet")
        );
    }

    #[test]
    fn duplicates_reorders_and_out_of_stage_entries_normalise_exact_sets() {
        let expected = vec!["a".to_owned(), "b".to_owned()];
        let not_judged = vec!["x".to_owned()];
        let mut raw = json!({"criteria": [
            {"criterion_id": "B", "judgment": "Met"},
            {"criterion_id": "x", "judgment": "meets_does_not_apply"},
            {"criterion_id": " a ", "judgment": "does_not_meet"},
            {"criterion_id": "b", "judgment": "met"},
            {"criterion_id": "a", "judgment": "does_not_meet"},
        ]});
        let report = normalize_keyed_judgments(&mut raw, &spec(&expected, &not_judged));
        assert_eq!(
            raw,
            json!({"criteria": [
                {"criterion_id": "a", "judgment": "does_not_meet"},
                {"criterion_id": "b", "judgment": "meets"},
            ]})
        );
        assert_eq!(report.not_judged_dropped, 1);
        assert_eq!(report.duplicates_dropped, 2);
        assert!(report.reordered);
    }

    #[test]
    fn missing_and_conflicting_entries_are_left_for_validation() {
        let expected = vec!["a".to_owned(), "b".to_owned()];
        let mut missing = json!({"criteria": [{"criterion_id": "a", "judgment": "meets"}]});
        let before = missing.clone();
        normalize_keyed_judgments(&mut missing, &spec(&expected, &[]));
        assert_eq!(missing, before);

        let mut conflicting = json!({"criteria": [
            {"criterion_id": "a", "judgment": "meets"},
            {"criterion_id": "a", "judgment": "does_not_meet"},
            {"criterion_id": "b", "judgment": "unclear"},
        ]});
        let before = conflicting.clone();
        let report = normalize_keyed_judgments(&mut conflicting, &spec(&expected, &[]));
        assert_eq!(conflicting, before);
        assert_eq!(report.duplicates_dropped, 0);
    }
}
