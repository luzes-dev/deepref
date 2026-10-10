//! Plan consistency: detecting replies that claim a change was queued when no
//! plan action exists, and correcting them in the user's language.
//!
//! The NL heuristics below are intentionally dumb string matchers, not a
//! classifier: their exact outcomes are pinned by tests (real claims in
//! English and Portuguese, denials, near-misses), so keep them in sync with
//! the fixtures there when touching the phrase tables.

/// Positive phrases that tell the user something was queued, planned or is
/// waiting for their confirmation. Matched per clause against the lower-cased
/// reply, in English and Portuguese, and only when the turn created no plan.
pub(crate) const PENDING_CHANGE_CLAIMS: &[&str] = &[
    "i've queued",
    "i have queued",
    "queued a plan",
    "queued the",
    "queued this",
    "queued it",
    "queued these",
    "queued for",
    "queued in",
    "is queued",
    "are queued",
    "in the plan",
    "to the plan",
    "the plan below",
    "prepared a plan",
    "i've prepared",
    "i have prepared",
    "confirm to apply",
    "once you confirm",
    "after you confirm",
    "when you confirm",
    "waiting for your confirmation",
    "awaiting your confirmation",
    "waiting for your ok",
    "will only run once",
    "will only run after",
    "na fila",
    "enfileir",
    "ao plano",
    "plano pendente",
    "preparei um plano",
    "preparei o plano",
    "aguardando sua confirma",
    "aguardando a sua confirma",
    "aguardando confirma",
    "quando voc\u{ea} confirmar",
    "depois que voc\u{ea} confirmar",
    "confirme para aplicar",
];

/// Words that turn a clause into a denial, such as "nothing has been queued".
pub(crate) const NEGATIONS: &[&str] = &[
    "nothing", " not ", "n't", " no ", "never", "none", "nada", "n\u{e3}o", "nenhum",
];

pub(crate) const NO_PLAN_CORRECTION: &str = "[System correction] Your last reply says something was queued, planned or is waiting for confirmation, but no change tool was called in this turn, so nothing is in a plan. Do not say that anything was queued. If the user wants a change, call the right change tool now. Otherwise reply again in the user's language and say plainly that nothing has been changed.";

/// True when a reply that created no plan nevertheless tells the user that
/// something was queued, planned or is waiting for their confirmation. A
/// clause that denies it ("nothing has been queued") is not a claim.
pub fn claims_pending_change(reply: &str) -> bool {
    reply
        .to_lowercase()
        .split(['.', '!', '?', ';', ':', '\n', '\u{2014}', '\u{2013}'])
        .any(|clause| {
            let padded = format!(" {clause} ");
            PENDING_CHANGE_CLAIMS
                .iter()
                .any(|claim| padded.contains(claim))
                && !NEGATIONS.iter().any(|word| padded.contains(word))
        })
}

/// The correction appended to a reply that still claims a change after the
/// one allowed re-prompt. Written in the language the user is using.
pub fn no_plan_notice(user_message: &str) -> &'static str {
    if looks_portuguese(user_message) {
        "Nada foi enfileirado: nenhuma altera\u{e7}\u{e3}o foi colocada em um plano, ent\u{e3}o nada aguarda a sua confirma\u{e7}\u{e3}o."
    } else {
        "Nothing was queued: no change was added to a plan, so nothing is waiting for your confirmation."
    }
}

fn looks_portuguese(text: &str) -> bool {
    const PORTUGUESE: &[&str] = &[
        " n\u{e3}o ",
        " que ",
        " para ",
        " uma ",
        " voc\u{ea}",
        " s\u{e3}o ",
        " dos ",
        " das ",
        " com ",
        "\u{e7}\u{e3}o",
    ];
    const ENGLISH: &[&str] = &[
        " the ", " and ", " what ", " which ", " how ", " is ", " are ",
    ];
    let padded = format!(" {} ", text.to_lowercase());
    let score = |markers: &[&str]| markers.iter().filter(|m| padded.contains(*m)).count();
    score(PORTUGUESE) > score(ENGLISH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_detection_reads_english_and_portuguese_only_for_real_claims() {
        assert!(claims_pending_change("I've queued a plan to propose that."));
        assert!(claims_pending_change(
            "Nothing has changed yet: this is in the plan."
        ));
        assert!(claims_pending_change(
            "Vou colocar na fila; confirme para aplicar."
        ));
        assert!(claims_pending_change(
            "Preparei um plano. Nada foi alterado ainda."
        ));
        assert!(!claims_pending_change("There are 50 unscreened records."));
        assert!(!claims_pending_change(
            "H\u{e1} 50 registros sem triagem ainda."
        ));
        assert!(!claims_pending_change(
            "The protocol plan has four criteria."
        ));
        // Denials are honest replies, not claims.
        assert!(!claims_pending_change(
            "I could not find a change to make, so nothing has been queued."
        ));
        assert!(!claims_pending_change("Nada foi enfileirado."));
    }

    #[test]
    fn correction_notice_follows_the_user_language() {
        assert!(
            no_plan_notice("Quantos registros ainda n\u{e3}o foram triados?")
                .starts_with("Nada foi enfileirado")
        );
        assert!(
            no_plan_notice("How many records are left to screen?")
                .starts_with("Nothing was queued")
        );
    }
}
