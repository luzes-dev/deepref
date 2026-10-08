//! Routing records by the AI's screening verdict.
//!
//! The "Run an AI review" block, when it screens, waits until the AI reviews it
//! started have finished and then sends every record out on the output that
//! matches the AI's answer: `included`, `excluded` or `unsure`. The verdict is
//! only a route. It never writes a screening decision; the AI's opinion is
//! recorded exactly as the project's autonomy setting says for any AI screening.
//!
//! Everything here is pure. The worker reads the reviews and does the waiting.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a screening step waits for the AI before it routes what is left as
/// "not sure".
pub const SCREENING_WAIT_SECS: i64 = 30 * 60;

/// Seconds between two looks at the AI reviews while a step waits.
pub const SCREENING_POLL_SECS: u64 = 20;

/// Records one screening step sends to the AI.
pub const SCREENING_BATCH: usize = 50;

const RATIONALE_CHARS: usize = 400;
const CRITERION_CHARS: usize = 240;
const MAX_CRITERIA_IN_RATIONALE: usize = 3;

/// The three answers a record can be routed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiVerdict {
    Include,
    Exclude,
    Unsure,
}

impl AiVerdict {
    /// The value written into `ai_decision` on each record.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::Exclude => "exclude",
            Self::Unsure => "unsure",
        }
    }

    /// The output port a record with this verdict leaves on.
    pub const fn port(self) -> &'static str {
        match self {
            Self::Include => "included",
            Self::Exclude => "excluded",
            Self::Unsure => "unsure",
        }
    }
}

/// The verdict for one record, with a short reason a person can read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub decision: AiVerdict,
    pub rationale: String,
}

impl Verdict {
    fn unsure(rationale: impl Into<String>) -> Self {
        Self {
            decision: AiVerdict::Unsure,
            rationale: truncate(&rationale.into(), RATIONALE_CHARS),
        }
    }

    /// The AI answered. `payload` is the screening analysis stored with the
    /// proposal: `suggested_decision` and one judgment per criterion.
    pub fn from_answer(payload: &Value) -> Self {
        let kind = payload
            .pointer("/suggested_decision/kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let reasons = criteria_rationale(payload);
        match kind {
            "include" => Self {
                decision: AiVerdict::Include,
                rationale: reasons.unwrap_or_else(|| {
                    "The AI judged that this record meets the protocol.".to_owned()
                }),
            },
            "exclude" => Self {
                decision: AiVerdict::Exclude,
                rationale: reasons.unwrap_or_else(|| {
                    "The AI judged that this record does not meet the protocol.".to_owned()
                }),
            },
            "maybe" => Self::unsure(match reasons {
                Some(reasons) => format!("The AI was not sure. {reasons}"),
                None => "The AI was not sure about this record.".to_owned(),
            }),
            "insufficient_evidence" => Self::unsure(match first_uncertainty(payload) {
                Some(text) => format!("The AI did not have enough information to decide. {text}"),
                None => "The AI did not have enough information to decide.".to_owned(),
            }),
            _ => Self::unsure("The AI's answer could not be read."),
        }
    }

    /// The review ended without an answer; the reason says why.
    pub fn stopped(reason: &str) -> Self {
        Self::unsure(reason)
    }

    /// The review was still running when the wait ran out.
    pub fn timed_out() -> Self {
        Self::unsure(format!(
            "The AI did not finish reviewing this record within {} minutes.",
            SCREENING_WAIT_SECS / 60
        ))
    }

    /// The record was not sent to the AI; the reason says why.
    pub fn not_reviewed(reason: &str) -> Self {
        Self::unsure(reason)
    }
}

fn criteria_rationale(payload: &Value) -> Option<String> {
    let parts: Vec<String> = payload
        .get("criteria")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|criterion| criterion.get("rationale").and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .take(MAX_CRITERIA_IN_RATIONALE)
        .map(|text| truncate(text, CRITERION_CHARS))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn first_uncertainty(payload: &Value) -> Option<String> {
    payload
        .get("uncertainties")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| truncate(text, CRITERION_CHARS))
}

/// Shorten by characters, never by bytes, so text is never cut mid-character.
pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut shortened: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

/// The record with its verdict written on it, so later steps and templates can
/// use `{{ai_decision}}` and `{{ai_rationale}}`.
pub fn annotate(record: Value, verdict: &Verdict) -> Value {
    match record {
        Value::Object(mut map) => {
            map.insert("ai_decision".to_owned(), json!(verdict.decision.as_str()));
            map.insert("ai_rationale".to_owned(), json!(verdict.rationale));
            Value::Object(map)
        }
        other => other,
    }
}

/// The records of one screening step, grouped by the port they leave on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Routed {
    /// Every record, annotated, after the wait.
    pub records: Vec<Value>,
    pub included: Vec<Value>,
    pub excluded: Vec<Value>,
    pub unsure: Vec<Value>,
}

impl Routed {
    /// Route each record by its verdict.
    pub fn route(judged: Vec<(Value, Verdict)>) -> Self {
        let mut routed = Self::default();
        for (record, verdict) in judged {
            let annotated = annotate(record, &verdict);
            routed.records.push(annotated.clone());
            match verdict.decision {
                AiVerdict::Include => routed.included.push(annotated),
                AiVerdict::Exclude => routed.excluded.push(annotated),
                AiVerdict::Unsure => routed.unsure.push(annotated),
            }
        }
        routed
    }

    pub fn count(&self, decision: AiVerdict) -> usize {
        match decision {
            AiVerdict::Include => self.included.len(),
            AiVerdict::Exclude => self.excluded.len(),
            AiVerdict::Unsure => self.unsure.len(),
        }
    }

    /// The ports that carry data: `records` always, a verdict port only when
    /// at least one record left on it.
    pub fn fired_ports(&self) -> Vec<String> {
        let mut fired = vec!["records".to_owned()];
        for decision in [AiVerdict::Include, AiVerdict::Exclude, AiVerdict::Unsure] {
            if self.count(decision) > 0 {
                fired.push(decision.port().to_owned());
            }
        }
        fired
    }
}

/// Judge every record of a step. A record without a matching review gets
/// "not sure" with the reason it was not reviewed; a reviewed record that is
/// still open at the end of the wait gets "not sure" too.
pub fn judge_records(records: Vec<Value>, reviews: &[ReviewSlot]) -> Vec<(Value, Verdict)> {
    records
        .into_iter()
        .map(|record| {
            let report_id = record
                .get("report_id")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok());
            let verdict = match report_id {
                None => Verdict::not_reviewed(
                    "This record could not be matched to a record in the project, so the AI did not review it.",
                ),
                Some(id) => match reviews.iter().find(|slot| slot.report_id == id) {
                    Some(slot) => slot.verdict.clone().unwrap_or_else(Verdict::timed_out),
                    None => Verdict::not_reviewed(&format!(
                        "A step reviews at most {SCREENING_BATCH} records at a time, so the AI did not review this one."
                    )),
                },
            };
            (record, verdict)
        })
        .collect()
}

/// One AI review started for one record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSlot {
    pub report_id: Uuid,
    /// `None` when the review could not be started (the verdict says why).
    pub run_id: Option<Uuid>,
    /// Set once the AI answered or the review ended. Open reviews have none.
    pub verdict: Option<Verdict>,
}

/// What a screening step keeps while it waits for the AI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreeningWait {
    pub deadline: DateTime<Utc>,
    /// How many times the step has been put back to wait.
    pub polls: u32,
    pub reviews: Vec<ReviewSlot>,
}

impl ScreeningWait {
    pub fn start(now: DateTime<Utc>, reviews: Vec<ReviewSlot>) -> Self {
        Self {
            deadline: now + Duration::seconds(SCREENING_WAIT_SECS),
            polls: 0,
            reviews,
        }
    }

    /// Reviews still open.
    pub fn open(&self) -> usize {
        self.reviews
            .iter()
            .filter(|slot| slot.verdict.is_none())
            .count()
    }

    /// Nothing left to wait for: every review settled, or the time ran out.
    pub fn is_over(&self, now: DateTime<Utc>) -> bool {
        self.open() == 0 || now >= self.deadline
    }
}

/// The note shown on the step while it waits.
pub fn progress_note(done: usize, total: usize) -> String {
    format!("Waiting for the AI to review {total} record(s): {done} of {total} finished so far.")
}

/// The note shown on the step when it finishes, with the count per output.
pub fn summary_note(routed: &Routed, timed_out: usize) -> String {
    let mut note = format!(
        "AI verdicts: {} would include, {} would exclude, {} not sure.",
        routed.count(AiVerdict::Include),
        routed.count(AiVerdict::Exclude),
        routed.count(AiVerdict::Unsure),
    );
    if timed_out > 0 {
        note.push_str(&format!(
            " {timed_out} record(s) were still open after {} minutes, so they went to Not sure.",
            SCREENING_WAIT_SECS / 60
        ));
    }
    note
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn answer(kind: &str, rationale: &str) -> Value {
        json!({
            "suggested_decision": {"kind": kind},
            "criteria": [
                {"criterion_id": Uuid::nil(), "judgment": "meets", "rationale": rationale, "evidence": []}
            ],
            "uncertainties": ["The abstract does not give the sample size."],
        })
    }

    fn record(report_id: Uuid, title: &str) -> Value {
        json!({"report_id": report_id.to_string(), "title": title})
    }

    #[test]
    fn answers_map_to_the_three_routes() {
        let include = Verdict::from_answer(&answer("include", "Adults with auditory neuropathy."));
        assert_eq!(include.decision, AiVerdict::Include);
        assert_eq!(include.rationale, "Adults with auditory neuropathy.");

        let exclude = Verdict::from_answer(&answer("exclude", "Animal study."));
        assert_eq!(exclude.decision, AiVerdict::Exclude);

        let maybe = Verdict::from_answer(&answer("maybe", "Unclear population."));
        assert_eq!(maybe.decision, AiVerdict::Unsure);
        assert!(maybe.rationale.starts_with("The AI was not sure."));

        let thin = Verdict::from_answer(&json!({
            "suggested_decision": {"kind": "insufficient_evidence"},
            "uncertainties": ["No abstract."],
        }));
        assert_eq!(thin.decision, AiVerdict::Unsure);
        assert!(thin.rationale.contains("No abstract."));

        let unreadable = Verdict::from_answer(&json!({"suggested_decision": {"kind": "??"}}));
        assert_eq!(unreadable.decision, AiVerdict::Unsure);
    }

    #[test]
    fn stopped_and_timed_out_reviews_are_not_sure_with_a_reason() {
        assert_eq!(
            Verdict::stopped("The AI budget is used up.").decision,
            AiVerdict::Unsure
        );
        let late = Verdict::timed_out();
        assert_eq!(late.decision, AiVerdict::Unsure);
        assert!(late.rationale.contains("30 minutes"), "{}", late.rationale);
    }

    #[test]
    fn records_leave_on_the_port_of_their_verdict_and_carry_it() {
        let (a, b, c, d) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let reviews = vec![
            ReviewSlot {
                report_id: a,
                run_id: Some(Uuid::new_v4()),
                verdict: Some(Verdict::from_answer(&answer("include", "Matches."))),
            },
            ReviewSlot {
                report_id: b,
                run_id: Some(Uuid::new_v4()),
                verdict: Some(Verdict::from_answer(&answer("exclude", "Wrong design."))),
            },
            ReviewSlot {
                report_id: c,
                run_id: Some(Uuid::new_v4()),
                verdict: None,
            },
        ];
        let records = vec![
            record(a, "A"),
            record(b, "B"),
            record(c, "C"),
            record(d, "D beyond the batch"),
            json!({"title": "No identifier"}),
        ];
        let routed = Routed::route(judge_records(records, &reviews));

        assert_eq!(routed.records.len(), 5);
        assert_eq!(routed.included.len(), 1);
        assert_eq!(routed.included[0]["ai_decision"], "include");
        assert_eq!(routed.included[0]["ai_rationale"], "Matches.");
        assert_eq!(routed.excluded.len(), 1);
        assert_eq!(routed.excluded[0]["title"], "B");
        // Open at the end of the wait, beyond the batch, or unmatched: not sure.
        assert_eq!(routed.unsure.len(), 3);
        assert!(
            routed.unsure[0]["ai_rationale"]
                .as_str()
                .is_some_and(|text| text.contains("30 minutes"))
        );
        assert!(
            routed.unsure[1]["ai_rationale"]
                .as_str()
                .is_some_and(|text| text.contains("at most 50"))
        );
        assert_eq!(
            routed.fired_ports(),
            vec![
                "records".to_owned(),
                "included".to_owned(),
                "excluded".to_owned(),
                "unsure".to_owned()
            ]
        );
        assert_eq!(
            summary_note(&routed, 1),
            "AI verdicts: 1 would include, 1 would exclude, 3 not sure. 1 record(s) were still open after 30 minutes, so they went to Not sure."
        );
    }

    #[test]
    fn empty_verdict_ports_do_not_fire() {
        let routed = Routed::route(vec![(
            json!({"title": "Only exclusions"}),
            Verdict::from_answer(&answer("exclude", "No.")),
        )]);
        assert!(routed.included.is_empty());
        assert_eq!(
            routed.fired_ports(),
            vec!["records".to_owned(), "excluded".to_owned()]
        );
    }

    #[test]
    fn wait_is_over_when_settled_or_past_the_deadline() {
        let now = Utc::now();
        let mut wait = ScreeningWait::start(
            now,
            vec![ReviewSlot {
                report_id: Uuid::new_v4(),
                run_id: Some(Uuid::new_v4()),
                verdict: None,
            }],
        );
        assert!(!wait.is_over(now));
        assert!(wait.is_over(now + Duration::seconds(SCREENING_WAIT_SECS)));
        wait.reviews[0].verdict = Some(Verdict::stopped("done"));
        assert!(wait.is_over(now));

        let round_trip: ScreeningWait =
            serde_json::from_value(serde_json::to_value(&wait).expect("encode")).expect("decode");
        assert_eq!(round_trip, wait);
    }

    #[test]
    fn truncation_counts_characters() {
        assert_eq!(truncate("ação", 3), "aç…");
        assert_eq!(truncate("short", 10), "short");
    }
}
