#![allow(clippy::expect_used, clippy::unwrap_used)]

//! The RoB 2 tool's answer shapes go through the same pre-fill validator the model output
//! does. These tests use a canned model output, so they run without a live AI provider.

use std::collections::BTreeMap;

use deepref_ai::{
    AiTask, AppraisalAnswerSchema, AppraisalAnswerValue, AppraisalConditionMatch, AppraisalPrefill,
    AppraisalPrefillAnswer, AppraisalPrefillClause, AppraisalPrefillCondition,
    AppraisalPrefillDomain, AppraisalPrefillEvidence, AppraisalPrefillInput,
    AppraisalPrefillQuestion, AppraisalPrefillTask,
};
use deepref_domain::{ProjectId, ReportId};
use uuid::Uuid;

const LOW: &str = "low_risk";
const SOME: &str = "some_concerns";
const HIGH: &str = "high_risk";

fn yes_no_options(with_not_applicable: bool) -> AppraisalAnswerSchema {
    let mut options = vec!["yes", "probably_yes", "probably_no", "no", "no_information"];
    if with_not_applicable {
        options.push("not_applicable");
    }
    AppraisalAnswerSchema::Enum {
        options: options.into_iter().map(str::to_owned).collect(),
    }
}

fn question(id: &str, with_not_applicable: bool) -> AppraisalPrefillQuestion {
    AppraisalPrefillQuestion {
        id: id.to_owned(),
        label: format!("Question {id}"),
        help: Some("Answer not applicable only when the question does not apply.".to_owned()),
        answer_schema: yes_no_options(with_not_applicable),
        required: true,
        requires_evidence: false,
        applies_when: None,
    }
}

fn domain(id: &str) -> AppraisalPrefillDomain {
    AppraisalPrefillDomain {
        id: id.to_owned(),
        label: format!("Domain {id}"),
        description: None,
        allowed_judgments: vec![LOW.to_owned(), SOME.to_owned(), HIGH.to_owned()],
        required: true,
    }
}

struct Fixture {
    input: AppraisalPrefillInput,
    evidence: AppraisalPrefillEvidence,
    report_id: ReportId,
}

fn fixture() -> Fixture {
    let report_id = ReportId::new(Uuid::from_u128(201));
    let evidence = AppraisalPrefillEvidence {
        document_id: Uuid::from_u128(202),
        document_block_id: Uuid::from_u128(203),
        page: 3,
        parser_version: "parser.v1".to_owned(),
        content_hash: "c".repeat(64),
    };
    let input = AppraisalPrefillInput {
        project_id: ProjectId::new(Uuid::from_u128(200)),
        report_id,
        definition_id: "deepref-rct-rob2".to_owned(),
        definition_version: 1,
        questions: vec![
            question("sequence_unpredictable", false),
            question("missing_data_method", true),
            question("assessors_unaware", true),
        ],
        domains: vec![
            domain("randomization_process"),
            domain("missing_outcome_data"),
        ],
        overall_allowed_judgments: vec![LOW.to_owned(), SOME.to_owned(), HIGH.to_owned()],
        report_title: Some("A randomized trial".to_owned()),
        report_abstract: None,
        grounded_evidence: vec![evidence.clone()],
        passages: Vec::new(),
    };
    Fixture {
        input,
        evidence,
        report_id,
    }
}

fn answer(
    question_id: &str,
    value: &str,
    evidence: Vec<AppraisalPrefillEvidence>,
) -> AppraisalPrefillAnswer {
    AppraisalPrefillAnswer {
        question_id: question_id.to_owned(),
        answer: AppraisalAnswerValue::Enum {
            value: value.to_owned(),
        },
        rationale: format!("Rationale for {question_id}."),
        evidence,
    }
}

fn output(fixture: &Fixture, answers: Vec<AppraisalPrefillAnswer>) -> AppraisalPrefill {
    AppraisalPrefill {
        report_id: fixture.report_id.as_uuid(),
        definition_id: "deepref-rct-rob2".to_owned(),
        definition_version: 1,
        answers,
        domain_judgments: BTreeMap::from([
            ("randomization_process".to_owned(), LOW.to_owned()),
            ("missing_outcome_data".to_owned(), SOME.to_owned()),
        ]),
        overall_judgment: SOME.to_owned(),
        override_reasons: BTreeMap::new(),
    }
}

fn valid_answers(fixture: &Fixture) -> Vec<AppraisalPrefillAnswer> {
    vec![
        answer(
            "sequence_unpredictable",
            "yes",
            vec![fixture.evidence.clone()],
        ),
        // "not applicable" is valid only where the question offers it.
        answer("missing_data_method", "not_applicable", Vec::new()),
        // "no information" is a valid answer and needs no supporting evidence.
        answer("assessors_unaware", "no_information", Vec::new()),
    ]
}

#[test]
fn rob2_shaped_output_with_not_applicable_and_no_information_validates() {
    let fixture = fixture();
    let task = AppraisalPrefillTask::new(&fixture.input).expect("rob2 prefill context");
    let valid = output(&fixture, valid_answers(&fixture));
    task.semantic_validate(&valid)
        .expect("rob2 answers and judgments should validate");
}

#[test]
fn rob2_output_rejects_answers_and_judgments_outside_the_tool() {
    let fixture = fixture();
    let task = AppraisalPrefillTask::new(&fixture.input).expect("rob2 prefill context");

    let mut unknown_answer = valid_answers(&fixture);
    unknown_answer[2] = answer("assessors_unaware", "maybe", Vec::new());
    assert!(
        task.semantic_validate(&output(&fixture, unknown_answer))
            .is_err(),
        "an answer outside the schema must be rejected"
    );

    let mut not_applicable_where_not_offered = valid_answers(&fixture);
    not_applicable_where_not_offered[0] = answer(
        "sequence_unpredictable",
        "not_applicable",
        vec![fixture.evidence.clone()],
    );
    assert!(
        task.semantic_validate(&output(&fixture, not_applicable_where_not_offered))
            .is_err(),
        "not applicable is only valid for questions that offer it"
    );

    let mut generic_judgment = output(&fixture, valid_answers(&fixture));
    generic_judgment
        .domain_judgments
        .insert("randomization_process".to_owned(), "low".to_owned());
    assert!(
        task.semantic_validate(&generic_judgment).is_err(),
        "judgments must use the tool's values"
    );

    let mut bad_overall = output(&fixture, valid_answers(&fixture));
    bad_overall.overall_judgment = "moderate".to_owned();
    assert!(
        task.semantic_validate(&bad_overall).is_err(),
        "the overall judgment must use the tool's values"
    );
}

#[test]
fn override_reasons_are_reviewer_only_and_absent_from_model_output() {
    let fixture = fixture();
    let mut prefill = output(&fixture, valid_answers(&fixture));
    let model_json = serde_json::to_value(&prefill).expect("prefill serializes");
    assert!(
        model_json.get("override_reasons").is_none(),
        "an empty reviewer field must not appear in the model output"
    );

    prefill.override_reasons = BTreeMap::from([(
        "randomization_process".to_owned(),
        "The reviewer read the sequence method in the protocol.".to_owned(),
    )]);
    let reviewed_json = serde_json::to_value(&prefill).expect("reviewed prefill serializes");
    assert_eq!(
        reviewed_json["override_reasons"]["randomization_process"],
        "The reviewer read the sequence method in the protocol."
    );
    let round_trip: AppraisalPrefill =
        serde_json::from_value(reviewed_json).expect("reviewed prefill deserializes");
    assert_eq!(round_trip, prefill);
    let schema =
        serde_json::to_string(&schemars::schema_for!(AppraisalPrefill)).expect("schema serializes");
    assert!(
        !schema.contains("override_reasons"),
        "the model's JSON schema must not ask for reviewer reasons"
    );
}

/// RoB 2 version 2: 2.3 is asked when 2.1 or 2.2 calls for it, and is not applicable otherwise.
fn yes_no_question(id: &str) -> AppraisalPrefillQuestion {
    AppraisalPrefillQuestion {
        id: id.to_owned(),
        label: format!("Question {id}"),
        help: None,
        answer_schema: yes_no_options(false),
        required: true,
        requires_evidence: false,
        applies_when: None,
    }
}

fn asked_when_aware(id: &str) -> AppraisalPrefillQuestion {
    AppraisalPrefillQuestion {
        id: id.to_owned(),
        label: format!("Question {id}"),
        help: None,
        answer_schema: yes_no_options(true),
        required: true,
        requires_evidence: false,
        applies_when: Some(AppraisalPrefillCondition {
            match_mode: AppraisalConditionMatch::Any,
            clauses: ["participants_aware", "personnel_aware"]
                .into_iter()
                .map(|question_id| AppraisalPrefillClause {
                    question_id: question_id.to_owned(),
                    answers: ["yes", "probably_yes", "no_information"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                })
                .collect(),
        }),
    }
}

fn conditional_input() -> AppraisalPrefillInput {
    AppraisalPrefillInput {
        project_id: ProjectId::new(Uuid::from_u128(300)),
        report_id: ReportId::new(Uuid::from_u128(301)),
        definition_id: "deepref-rct-rob2".to_owned(),
        definition_version: 2,
        questions: vec![
            yes_no_question("participants_aware"),
            yes_no_question("personnel_aware"),
            asked_when_aware("trial_context_deviations"),
        ],
        domains: Vec::new(),
        overall_allowed_judgments: vec![LOW.to_owned(), SOME.to_owned(), HIGH.to_owned()],
        report_title: None,
        report_abstract: None,
        grounded_evidence: Vec::new(),
        passages: Vec::new(),
    }
}

fn conditional_output(
    input: &AppraisalPrefillInput,
    participants: &str,
    personnel: &str,
    context: &str,
) -> AppraisalPrefill {
    AppraisalPrefill {
        report_id: input.report_id.as_uuid(),
        definition_id: input.definition_id.clone(),
        definition_version: input.definition_version,
        answers: vec![
            answer("participants_aware", participants, Vec::new()),
            answer("personnel_aware", personnel, Vec::new()),
            answer("trial_context_deviations", context, Vec::new()),
        ],
        domain_judgments: BTreeMap::new(),
        overall_judgment: LOW.to_owned(),
        override_reasons: BTreeMap::new(),
    }
}

#[test]
fn conditional_question_must_be_not_applicable_when_its_condition_fails() {
    let input = conditional_input();
    let task = AppraisalPrefillTask::new(&input).expect("rob2 prefill context");
    // Neither awareness question calls for 2.3, so not applicable is the only valid answer.
    task.semantic_validate(&conditional_output(
        &input,
        "no",
        "probably_no",
        "not_applicable",
    ))
    .expect("not applicable when the condition fails");
    assert!(
        task.semantic_validate(&conditional_output(&input, "no", "probably_no", "yes"))
            .is_err(),
        "a question outside its condition must not be answered"
    );
}

#[test]
fn conditional_question_must_be_answered_when_its_condition_holds() {
    let input = conditional_input();
    let task = AppraisalPrefillTask::new(&input).expect("rob2 prefill context");
    task.semantic_validate(&conditional_output(&input, "yes", "no", "probably_no"))
        .expect("answered when the condition holds");
    assert!(
        task.semantic_validate(&conditional_output(&input, "yes", "no", "not_applicable"))
            .is_err(),
        "a question whose condition holds must not be not applicable"
    );
}

#[test]
fn normalize_output_fills_evidence_fields_from_the_grounded_block() {
    let fixture = fixture();
    let task = AppraisalPrefillTask::new(&fixture.input).expect("rob2 prefill context");
    let mut raw = serde_json::to_value(output(&fixture, valid_answers(&fixture)))
        .expect("prefill serializes");
    // The model cites the block by id only; the other fields come from the grounded block.
    let citation = &mut raw["answers"][0]["evidence"][0];
    let block_id = citation["document_block_id"].clone();
    *citation = serde_json::json!({ "document_block_id": block_id });
    task.normalize_output(&mut raw, &[]);
    assert_eq!(
        raw["answers"][0]["evidence"][0]["content_hash"],
        serde_json::json!(fixture.evidence.content_hash)
    );
    let output: AppraisalPrefill = serde_json::from_value(raw).expect("prefill deserializes");
    task.semantic_validate(&output)
        .expect("a citation completed from the grounded block validates");
}

#[test]
fn normalize_output_never_invents_a_citation_for_an_unknown_block() {
    let fixture = fixture();
    let task = AppraisalPrefillTask::new(&fixture.input).expect("rob2 prefill context");
    let mut raw = serde_json::to_value(output(&fixture, valid_answers(&fixture)))
        .expect("prefill serializes");
    raw["answers"][0]["evidence"][0] =
        serde_json::json!({ "document_block_id": Uuid::from_u128(999) });
    task.normalize_output(&mut raw, &[]);
    assert!(
        raw["answers"][0]["evidence"][0]
            .get("content_hash")
            .is_none(),
        "no hash is invented for a block that was not retrieved"
    );
    assert!(
        serde_json::from_value::<AppraisalPrefill>(raw).is_err(),
        "an incomplete citation is not a valid prefill"
    );
}

#[test]
fn a_missing_citation_names_the_question_for_the_corrective_reask() {
    let mut fixture = fixture();
    fixture.input.questions[0].requires_evidence = true;
    let task = AppraisalPrefillTask::new(&fixture.input).expect("rob2 prefill context");
    let mut answers = valid_answers(&fixture);
    answers[0].evidence.clear();
    let error = task
        .semantic_validate(&output(&fixture, answers))
        .expect_err("a required citation is missing");
    // The display text is generic; the reason is what the corrective re-ask receives.
    let reason = match error {
        deepref_ai::AiError::SemanticValidation(reason) => reason,
        other => panic!("expected a semantic validation error, got {other:?}"),
    };
    assert!(reason.contains("sequence_unpredictable"), "{reason}");
    assert!(reason.contains("cite"), "{reason}");
}

#[test]
fn appraisal_prefill_allows_one_corrective_reask_and_states_the_evidence_contract() {
    let fixture = fixture();
    let task = AppraisalPrefillTask::new(&fixture.input).expect("rob2 prefill context");
    assert_eq!(task.repair_attempts(), 1);
    let context = task.build_context(&fixture.input).expect("prefill context");
    assert!(context.system_prompt.contains("Evidence contract"));
    assert!(context.system_prompt.contains("Never invent"));
    assert!(
        !context
            .system_prompt
            .contains("Questions that require evidence"),
        "no question needs evidence in the RoB 2 fixture"
    );
}
