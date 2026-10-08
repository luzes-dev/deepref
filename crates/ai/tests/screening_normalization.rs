//! Screening normalisation, the contract sent to the model, and the repair
//! retry. The fixtures are judgments captured from the live model
//! (glm-5.3-flash, screening.title_abstract.v2, 2026-10-08) on the
//! "Wearables e atividade física" protocol. Rationales are shortened; criterion
//! ids, judgments, decisions and content hashes are as the model returned them.
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]

use std::{
    collections::{BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};

use deepref_domain::{
    CriterionDimension, CriterionKind, CriterionStage, DocumentBlockId, EligibilityCriterion,
    ProjectId,
};
use serde_json::{Value, json};
use uuid::Uuid;

use deepref_ai::*;

const PROJECT: &str = "33438758-42f0-4da2-9c3d-22a2c51f1109";
const PROTOCOL: &str = "7078a43b-8a57-487c-90ae-980a87ed0129";
const ADULTS: &str = "f6a951f3-5b58-460d-ac2e-4cfd40b7cee4";
const TRACKER: &str = "1c5b9d2b-9f4f-457e-a012-b0a4c0447f40";
const ACTIVITY: &str = "2bd1b493-7961-470f-bedb-78c1fd485a7d";
const NOT_PRIMARY: &str = "80856b83-b8a8-42db-92a0-af1f557141b4";
const VALIDITY_ONLY: &str = "a0106750-3e02-4e7e-9664-a7c9b5510210";

const REPORT_A: &str = "66693fb2-3ecf-4304-a4bb-ec35ae4a9436";
const REPORT_A_TITLE: &str = "2a2ffb3265393f6968e3df092850ce27327fc1a87582096b88d8a3739ee8a33a";
const REPORT_A_ABSTRACT: &str = "097b29d5da32d66084d0133a185202f82a5b508ad0ca61b91b7c071064b29537";
const REPORT_B: &str = "5bf03cce-9c11-45ab-b709-1d7ee201eeae";
const REPORT_B_ABSTRACT: &str = "d44dda1667a106d04ccfcb73bf07a4389f95057e288f20670f850e0d6ca37afe";
const REPORT_C: &str = "3c2324ac-8817-4a26-a74b-814ec9b48643";
const REPORT_C_ABSTRACT: &str = "71ae58ad9964e31e61d028a0549996b53a01ff0dc107b40c95e0310f1b83f062";
const REPORT_D: &str = "8deb1d92-cdef-4791-9c46-431a9edfaa55";
const REPORT_D_ABSTRACT: &str = "99ca14ae5028faaa9b27d9c60ef3938d5c99e474f06f574dbd4d41645b4f0369";
const REPORT_E: &str = "6dcb5869-5f6e-4480-afa8-6ecf22b5ef5a";
const REPORT_E_ABSTRACT: &str = "5f9c2e38c4d187f85e1d8ddb7fc8bc2861a1e8d160e84a9b775bf13a04476ac1";

fn id(text: &str) -> Uuid {
    text.parse().expect("fixture uuid")
}

fn criterion(
    text: &str,
    kind: CriterionKind,
    stage: CriterionStage,
    dimension: CriterionDimension,
    ordinal: i32,
) -> EligibilityCriterion {
    EligibilityCriterion::new(
        id(text),
        kind,
        stage,
        dimension,
        format!("Criterion {ordinal}"),
        "Protocol criterion".to_owned(),
        ordinal,
    )
    .expect("criterion")
}

/// The published protocol. ACTIVITY is judged at full text only, so title/abstract
/// screening must judge four criteria.
fn protocol() -> Vec<EligibilityCriterion> {
    vec![
        criterion(
            ADULTS,
            CriterionKind::Inclusion,
            CriterionStage::Both,
            CriterionDimension::Population,
            0,
        ),
        criterion(
            TRACKER,
            CriterionKind::Inclusion,
            CriterionStage::Both,
            CriterionDimension::Intervention,
            1,
        ),
        criterion(
            ACTIVITY,
            CriterionKind::Inclusion,
            CriterionStage::FullText,
            CriterionDimension::Outcome,
            2,
        ),
        criterion(
            NOT_PRIMARY,
            CriterionKind::Exclusion,
            CriterionStage::Both,
            CriterionDimension::Design,
            3,
        ),
        criterion(
            VALIDITY_ONLY,
            CriterionKind::Exclusion,
            CriterionStage::Both,
            CriterionDimension::Outcome,
            4,
        ),
    ]
}

/// A protocol whose tracker criterion is title/abstract only, for full-text tests.
fn full_text_protocol() -> Vec<EligibilityCriterion> {
    vec![
        criterion(
            TRACKER,
            CriterionKind::Inclusion,
            CriterionStage::TitleAbstract,
            CriterionDimension::Intervention,
            1,
        ),
        criterion(
            ACTIVITY,
            CriterionKind::Inclusion,
            CriterionStage::FullText,
            CriterionDimension::Outcome,
            2,
        ),
        criterion(
            NOT_PRIMARY,
            CriterionKind::Exclusion,
            CriterionStage::Both,
            CriterionDimension::Design,
            3,
        ),
    ]
}

fn prompt(criterion: &EligibilityCriterion) -> CriterionPrompt {
    CriterionPrompt {
        id: criterion.id,
        label: criterion.label.clone(),
        description: criterion.description.clone(),
        ordinal: criterion.ordinal,
        kind: match criterion.kind {
            CriterionKind::Inclusion => "inclusion",
            CriterionKind::Exclusion => "exclusion",
        }
        .to_owned(),
        stage: match criterion.stage {
            CriterionStage::TitleAbstract => "title_abstract",
            CriterionStage::FullText => "full_text",
            CriterionStage::Both => "both",
        }
        .to_owned(),
    }
}

fn metadata_evidence(report: &str, field: ScreeningEvidenceField, hash: &str) -> ScreeningEvidence {
    ScreeningEvidence::ReportMetadata {
        report_id: id(report),
        field,
        content_hash: hash.to_owned(),
    }
}

fn ta_task(report: &str, title_hash: &str, abstract_hash: &str) -> ScreeningTask {
    ScreeningTask::new(ScreeningTaskConfig {
        project_id: ProjectId::new(id(PROJECT)),
        report_id: id(report).into(),
        stage: ScreeningStage::TitleAbstract,
        protocol_version_id: id(PROTOCOL).into(),
        expected_revision: 0,
        criteria: protocol(),
        allowed_evidence: vec![
            metadata_evidence(report, ScreeningEvidenceField::Title, title_hash),
            metadata_evidence(report, ScreeningEvidenceField::Abstract, abstract_hash),
        ],
        allowed_exclusion_reasons: BTreeSet::new(),
    })
}

/// The whole protocol is sent as input, as production does. The task has to
/// narrow it to the criteria of its own stage.
fn ta_input(report: &str) -> ScreeningInput {
    ScreeningInput {
        project_id: ProjectId::new(id(PROJECT)),
        report_id: id(report).into(),
        stage: ScreeningStage::TitleAbstract,
        protocol_version_id: id(PROTOCOL).into(),
        expected_revision: 0,
        title: Some("Captured title".to_owned()),
        abstract_text: Some("Captured abstract".to_owned()),
        document_hash: None,
        retrieval_query: None,
        criteria: protocol().iter().map(prompt).collect(),
    }
}

fn judgment(criterion: &str, value: &str, rationale: &str, evidence: Vec<Value>) -> Value {
    json!({"criterion_id": criterion, "judgment": value, "rationale": rationale, "evidence": evidence})
}

fn metadata(report: &str, field: &str, hash: &str) -> Value {
    json!({"kind": "report_metadata", "report_id": report, "field": field, "content_hash": hash})
}

fn analysis(report: &str, criteria: Vec<Value>, decision: Value) -> Value {
    json!({
        "report_id": report,
        "expected_revision": 0,
        "stage": "title_abstract",
        "protocol_version_id": PROTOCOL,
        "criteria": criteria,
        "suggested_decision": decision,
        "uncertainties": ["Captured uncertainty."],
    })
}

/// Captured (report A): an exclusion that names a criterion id as its reason,
/// plus an extra judgment for the full-text-only criterion.
fn captured_a() -> Value {
    let abstract_a = || metadata(REPORT_A, "abstract", REPORT_A_ABSTRACT);
    analysis(
        REPORT_A,
        vec![
            judgment(
                ADULTS,
                "unclear",
                "Ages are not stated.",
                vec![abstract_a()],
            ),
            judgment(
                TRACKER,
                "does_not_meet",
                "No wearable tracker.",
                vec![abstract_a()],
            ),
            judgment(
                ACTIVITY,
                "unclear",
                "Assessed at full text.",
                vec![abstract_a()],
            ),
            judgment(
                NOT_PRIMARY,
                "meets",
                "Reads as a review.",
                vec![metadata(REPORT_A, "title", REPORT_A_TITLE), abstract_a()],
            ),
            judgment(
                VALIDITY_ONLY,
                "does_not_meet",
                "Not a validity study.",
                vec![abstract_a()],
            ),
        ],
        json!({"kind": "exclude", "exclusion_reason_id": NOT_PRIMARY}),
    )
}

/// Captured (report B): five judgments, the last one for the full-text-only
/// criterion, and an exclusion supported by the inclusion criteria.
fn captured_b() -> Value {
    let abstract_b = || metadata(REPORT_B, "abstract", REPORT_B_ABSTRACT);
    analysis(
        REPORT_B,
        vec![
            judgment(
                ADULTS,
                "does_not_meet",
                "Children aged 7 to 12.",
                vec![abstract_b()],
            ),
            judgment(
                TRACKER,
                "does_not_meet",
                "Remote microphones, not trackers.",
                vec![abstract_b()],
            ),
            judgment(
                ACTIVITY,
                "does_not_meet",
                "No steps or MVPA reported.",
                vec![abstract_b()],
            ),
            judgment(NOT_PRIMARY, "meets", "Empirical study.", vec![abstract_b()]),
            judgment(
                VALIDITY_ONLY,
                "does_not_meet",
                "Not a validity study.",
                vec![abstract_b()],
            ),
        ],
        json!({"kind": "exclude", "exclusion_reason_id": null}),
    )
}

/// Captured (report C): the exclusion judgment is a placeholder that the closed
/// vocabulary does not contain, and the last criterion is missing.
fn captured_c() -> Value {
    let abstract_c = || metadata(REPORT_C, "abstract", REPORT_C_ABSTRACT);
    analysis(
        REPORT_C,
        vec![
            judgment(
                ADULTS,
                "does_not_meet",
                "Children aged 7 to 10.",
                vec![abstract_c()],
            ),
            judgment(
                TRACKER,
                "does_not_meet",
                "No wearable tracker.",
                vec![abstract_c()],
            ),
            judgment(
                ACTIVITY,
                "does_not_meet",
                "Full-text criterion.",
                vec![abstract_c()],
            ),
            json!({"criterion_id": NOT_PRIMARY, "judgment": "meets_not_excluded_placeholder", "rationale": "PLACEHOLDER", "evidence": []}),
        ],
        json!({"kind": "exclude", "exclusion_reason_id": null}),
    )
}

/// Captured (report D): the last judgment has the value `does_not_meat` and no
/// rationale field at all.
fn captured_d() -> Value {
    let abstract_d = || metadata(REPORT_D, "abstract", REPORT_D_ABSTRACT);
    analysis(
        REPORT_D,
        vec![
            judgment(
                ADULTS,
                "does_not_meet",
                "Children with hearing loss.",
                vec![abstract_d()],
            ),
            judgment(
                TRACKER,
                "does_not_meet",
                "No tracker intervention.",
                vec![abstract_d()],
            ),
            judgment(
                ACTIVITY,
                "does_not_meet",
                "Auditory outcomes only.",
                vec![abstract_d()],
            ),
            judgment(
                NOT_PRIMARY,
                "does_not_meet",
                "Original empirical study.",
                vec![abstract_d()],
            ),
            json!({"criterion_id": VALIDITY_ONLY, "judgment": "does_not_meat", "evidence": [abstract_d()]}),
        ],
        json!({"kind": "exclude", "exclusion_reason_id": null}),
    )
}

/// Captured (report E): judgments without evidence for a non-insufficient
/// decision, and an exclusion reason that is not allowed at title/abstract.
fn captured_e() -> Value {
    let abstract_e = || metadata(REPORT_E, "abstract", REPORT_E_ABSTRACT);
    analysis(
        REPORT_E,
        vec![
            judgment(
                ADULTS,
                "does_not_meet",
                "Children with auditory processing.",
                vec![abstract_e()],
            ),
            judgment(
                TRACKER,
                "does_not_meet",
                "No activity tracker.",
                vec![abstract_e()],
            ),
            judgment(ACTIVITY, "unclear", "Full-text criterion.", vec![]),
            judgment(NOT_PRIMARY, "meets", "Primary study.", vec![abstract_e()]),
            judgment(
                VALIDITY_ONLY,
                "does_not_meet",
                "Not a validity study.",
                vec![],
            ),
        ],
        json!({"kind": "exclude", "exclusion_reason_id": NOT_PRIMARY}),
    )
}

fn criteria_mut(value: &mut Value) -> &mut Vec<Value> {
    value["criteria"].as_array_mut().expect("criteria array")
}

fn normalized(task: &ScreeningTask, mut raw: Value) -> Value {
    task.normalize_output(&mut raw, &[]);
    raw
}

fn typed(raw: Value) -> Result<ScreeningAnalysis, serde_json::Error> {
    serde_json::from_value(raw)
}

fn judged_ids(raw: &Value) -> Vec<String> {
    raw["criteria"]
        .as_array()
        .expect("criteria")
        .iter()
        .filter_map(|entry| entry["criterion_id"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn out_of_stage_judgment_and_title_abstract_reason_are_normalised_into_a_valid_exclusion() {
    let task = ta_task(REPORT_A, REPORT_A_TITLE, REPORT_A_ABSTRACT);
    let raw = captured_a();
    let as_typed = typed(raw.clone()).expect("captured answer is well-formed");
    assert_eq!(as_typed.criteria.len(), 5);
    assert!(
        task.semantic_validate(&as_typed).is_err(),
        "the captured answer must fail before normalisation"
    );

    let repaired = normalized(&task, raw);
    assert_eq!(
        repaired["suggested_decision"]["exclusion_reason_id"],
        Value::Null
    );
    assert_eq!(
        judged_ids(&repaired),
        vec![ADULTS, TRACKER, NOT_PRIMARY, VALIDITY_ONLY]
    );
    let output = typed(repaired).expect("normalised answer is well-formed");
    task.semantic_validate(&output)
        .expect("the exclusion is supported by an exclusion criterion");
}

#[test]
fn captured_exclusion_passes_after_dropping_the_out_of_stage_judgment() {
    let task = ta_task(REPORT_B, REPORT_B_ABSTRACT, REPORT_B_ABSTRACT);
    let repaired = normalized(&task, captured_b());
    assert_eq!(judged_ids(&repaired).len(), 4);
    task.semantic_validate(&typed(repaired).expect("typed"))
        .expect("four in-stage judgments validate");
}

#[test]
fn captured_placeholder_verdict_is_not_repaired() {
    let task = ta_task(REPORT_C, REPORT_C_ABSTRACT, REPORT_C_ABSTRACT);
    let repaired = normalized(&task, captured_c());
    assert_eq!(
        repaired["criteria"][2]["judgment"],
        "meets_not_excluded_placeholder"
    );
    assert!(
        typed(repaired).is_err(),
        "an unknown verdict and a missing judgment must never be invented"
    );
}

#[test]
fn captured_typo_is_mapped_but_a_missing_rationale_is_not_invented() {
    let task = ta_task(REPORT_D, REPORT_D_ABSTRACT, REPORT_D_ABSTRACT);
    let repaired = normalized(&task, captured_d());
    assert_eq!(repaired["criteria"][3]["judgment"], "does_not_meet");
    assert!(
        typed(repaired.clone()).is_err(),
        "a missing rationale stays a schema failure"
    );

    let mut with_rationale = repaired;
    with_rationale["criteria"][3]["rationale"] = json!("No validity study.");
    task.semantic_validate(&typed(with_rationale).expect("typed"))
        .expect("the typo-mapped judgment validates once the model supplies a rationale");
}

#[test]
fn reordered_and_exactly_duplicated_judgments_are_normalised() {
    let task = ta_task(REPORT_B, REPORT_B_ABSTRACT, REPORT_B_ABSTRACT);
    let mut raw = captured_b();
    let entries = criteria_mut(&mut raw);
    entries.reverse();
    let duplicate = entries[0].clone();
    entries.push(duplicate);
    entries[4]["criterion_id"] = json!(ADULTS.to_uppercase());
    let repaired = normalized(&task, raw);
    assert_eq!(
        judged_ids(&repaired),
        vec![ADULTS, TRACKER, NOT_PRIMARY, VALIDITY_ONLY]
    );
    task.semantic_validate(&typed(repaired).expect("typed"))
        .expect("reordered, de-duplicated judgments validate");
}

#[test]
fn missing_and_conflicting_judgments_are_rejected_for_repair() {
    let task = ta_task(REPORT_B, REPORT_B_ABSTRACT, REPORT_B_ABSTRACT);

    let mut missing = captured_b();
    criteria_mut(&mut missing).retain(|entry| entry["criterion_id"] != NOT_PRIMARY);
    let missing = normalized(&task, missing);
    assert!(
        task.semantic_validate(&typed(missing).expect("typed"))
            .is_err()
    );

    let mut conflicting = captured_b();
    let mut second = criteria_mut(&mut conflicting)[0].clone();
    second["judgment"] = json!("meets");
    criteria_mut(&mut conflicting).push(second);
    let conflicting = normalized(&task, conflicting);
    assert_eq!(
        judged_ids(&conflicting).len(),
        5,
        "a conflicting duplicate is kept"
    );
    assert!(
        task.semantic_validate(&typed(conflicting).expect("typed"))
            .is_err()
    );

    let mut unknown = captured_b();
    criteria_mut(&mut unknown)[1]["criterion_id"] = json!(Uuid::from_u128(999).to_string());
    let unknown = normalized(&task, unknown);
    assert!(
        task.semantic_validate(&typed(unknown).expect("typed"))
            .is_err()
    );
}

#[test]
fn not_applicable_answers_become_unclear_and_never_support_an_include() {
    let task = ta_task(REPORT_B, REPORT_B_ABSTRACT, REPORT_B_ABSTRACT);
    let abstract_b = || metadata(REPORT_B, "abstract", REPORT_B_ABSTRACT);
    let raw = json!({
        "report_id": REPORT_B,
        "expected_revision": 0,
        "stage": "title_abstract",
        "protocol_version_id": PROTOCOL,
        "criteria": [
            judgment(ADULTS, "meets", "Adults are described.", vec![abstract_b()]),
            judgment(TRACKER, "Not Applicable", "The tracker is not mentioned.", vec![abstract_b()]),
            judgment(NOT_PRIMARY, "does_not_meet", "Primary study.", vec![abstract_b()]),
            judgment(VALIDITY_ONLY, "meets_does_not_apply", "Not applicable.", vec![abstract_b()]),
        ],
        "suggested_decision": {"kind": "include"},
        "uncertainties": [],
    });
    let repaired = normalized(&task, raw);
    assert_eq!(repaired["criteria"][1]["judgment"], "unclear");
    assert_eq!(repaired["criteria"][3]["judgment"], "unclear");
    assert!(
        task.semantic_validate(&typed(repaired).expect("typed"))
            .is_err(),
        "an include must not rest on an unclear judgment"
    );
}

#[test]
fn captured_missing_evidence_is_not_invented() {
    let task = ta_task(REPORT_E, REPORT_E_ABSTRACT, REPORT_E_ABSTRACT);
    let repaired = normalized(&task, captured_e());
    assert_eq!(
        repaired["suggested_decision"]["exclusion_reason_id"],
        Value::Null
    );
    assert!(
        task.semantic_validate(&typed(repaired).expect("typed"))
            .is_err(),
        "a non-insufficient decision still needs evidence for every judgment"
    );
}

fn full_text_task() -> ScreeningTask {
    ScreeningTask::new(ScreeningTaskConfig {
        project_id: ProjectId::new(id(PROJECT)),
        report_id: id(REPORT_B).into(),
        stage: ScreeningStage::FullText,
        protocol_version_id: id(PROTOCOL).into(),
        expected_revision: 0,
        criteria: full_text_protocol(),
        allowed_evidence: Vec::new(),
        allowed_exclusion_reasons: BTreeSet::from([id(VALIDITY_ONLY)]),
    })
}

fn retrieved(block: &str, page: u32, hash: &str) -> GroundedBlock {
    GroundedBlock {
        evidence: EvidenceRef::new(DocumentBlockId::new(id(block)), page, hash.to_owned())
            .expect("evidence")
            .with_retrieval(1, 1.0)
            .expect("retrieval"),
        text: "Retrieved full-text block".to_owned(),
        retrieval_rank: 1,
        retrieval_score: 1.0,
    }
}

fn full_text_answer(citation: &Value) -> Value {
    json!({
        "report_id": REPORT_B,
        "expected_revision": 0,
        "stage": "full_text",
        "protocol_version_id": PROTOCOL,
        "criteria": [
            {"criterion_id": ACTIVITY, "judgment": "meets", "rationale": "Objective steps per day are reported.", "evidence": [citation]},
            {"criterion_id": NOT_PRIMARY, "judgment": "does_not_meet", "rationale": "Primary study.", "evidence": [citation]},
            {"criterion_id": TRACKER, "judgment": "meets", "rationale": "Judged at title/abstract stage only.", "evidence": [citation]},
        ],
        "suggested_decision": {"kind": "include"},
        "uncertainties": [],
    })
}

#[test]
fn full_text_citation_takes_the_hash_of_the_single_retrieved_block() {
    const BLOCK: &str = "df6bdda6-73e1-4f95-ad64-9ff680daceb5";
    const BLOCK_HASH: &str = "4b1f9e0c2d3a8f7e6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a";
    let task = full_text_task();
    // The model cites the block by the kind name it used in the captured failure.
    let citation = json!({"kind": "document_block_id", "document_block_id": BLOCK, "page": 2, "content_hash": null});
    let evidence = [retrieved(BLOCK, 2, BLOCK_HASH)];

    let mut normalised = full_text_answer(&citation);
    task.normalize_output(&mut normalised, &evidence);
    assert_eq!(judged_ids(&normalised), vec![ACTIVITY, NOT_PRIMARY]);
    assert_eq!(
        normalised["criteria"][0]["evidence"][0]["kind"],
        "document_block"
    );
    assert_eq!(
        normalised["criteria"][0]["evidence"][0]["content_hash"],
        BLOCK_HASH
    );
    task.semantic_validate_with_evidence(&typed(normalised).expect("typed"), &evidence)
        .expect("the full-text include validates against the retrieved block");

    let mut ambiguous = full_text_answer(&citation);
    let twins = [
        retrieved(BLOCK, 2, BLOCK_HASH),
        retrieved(BLOCK, 2, &"f".repeat(64)),
    ];
    task.normalize_output(&mut ambiguous, &twins);
    assert!(
        typed(ambiguous).is_err(),
        "two blocks with the same id and page leave the hash unfilled"
    );
}

#[test]
fn contract_names_only_the_criteria_of_the_stage_in_order() {
    let ta = ta_task(REPORT_A, REPORT_A_TITLE, REPORT_A_ABSTRACT);
    let context = ta
        .build_context(&ta_input(REPORT_A))
        .expect("title/abstract context");
    let user: Value = serde_json::from_str(&context.user_prompt).expect("user prompt is JSON");
    assert_eq!(
        user["output_contract"]["criterion_order"],
        json!([ADULTS, TRACKER, NOT_PRIMARY, VALIDITY_ONLY])
    );
    assert_eq!(
        user["output_contract"]["criteria_not_judged_at_this_stage"],
        json!([ACTIVITY])
    );
    let shown: Vec<&str> = user["input"]["criteria"]
        .as_array()
        .expect("input criteria")
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert_eq!(shown, vec![ADULTS, TRACKER, NOT_PRIMARY, VALIDITY_ONLY]);
    assert!(
        user["output_contract"]
            .get("exclusion_reason_ids_allowed")
            .is_none()
    );
    assert_eq!(ta.prompt_version(), "screening.title_abstract.v3");

    let ft = full_text_task();
    let ft_input = ScreeningInput {
        project_id: ProjectId::new(id(PROJECT)),
        report_id: id(REPORT_B).into(),
        stage: ScreeningStage::FullText,
        protocol_version_id: id(PROTOCOL).into(),
        expected_revision: 0,
        title: Some("Captured title".to_owned()),
        abstract_text: Some("Captured abstract".to_owned()),
        document_hash: None,
        retrieval_query: None,
        criteria: full_text_protocol().iter().map(prompt).collect(),
    };
    let context = ft.build_context(&ft_input).expect("full-text context");
    let user: Value = serde_json::from_str(&context.user_prompt).expect("user prompt is JSON");
    assert_eq!(
        user["output_contract"]["criterion_order"],
        json!([ACTIVITY, NOT_PRIMARY])
    );
    assert_eq!(
        user["output_contract"]["criteria_not_judged_at_this_stage"],
        json!([TRACKER])
    );
    assert_eq!(
        user["output_contract"]["exclusion_reason_ids_allowed"],
        json!([VALIDITY_ONLY])
    );
    assert_eq!(ft.prompt_version(), "screening.full_text.v2");
}

#[test]
fn the_shared_stage_filter_selects_the_criteria_each_stage_judges() {
    let all = protocol();
    let title_abstract: Vec<Uuid> = criteria_for_stage(&all, ScreeningStage::TitleAbstract)
        .iter()
        .map(|criterion| criterion.id)
        .collect();
    assert_eq!(
        title_abstract,
        vec![id(ADULTS), id(TRACKER), id(NOT_PRIMARY), id(VALIDITY_ONLY)]
    );
    let full_text: Vec<Uuid> = criteria_for_stage(&all, ScreeningStage::FullText)
        .iter()
        .map(|criterion| criterion.id)
        .collect();
    assert_eq!(
        full_text,
        vec![
            id(ADULTS),
            id(TRACKER),
            id(ACTIVITY),
            id(NOT_PRIMARY),
            id(VALIDITY_ONLY)
        ]
    );
}

#[test]
fn full_text_never_offers_title_abstract_metadata() {
    let task = ScreeningTask::new(ScreeningTaskConfig {
        project_id: ProjectId::new(id(PROJECT)),
        report_id: id(REPORT_B).into(),
        stage: ScreeningStage::FullText,
        protocol_version_id: id(PROTOCOL).into(),
        expected_revision: 0,
        criteria: full_text_protocol(),
        allowed_evidence: vec![metadata_evidence(
            REPORT_B,
            ScreeningEvidenceField::Abstract,
            REPORT_B_ABSTRACT,
        )],
        allowed_exclusion_reasons: BTreeSet::new(),
    });
    let input = ScreeningInput {
        project_id: ProjectId::new(id(PROJECT)),
        report_id: id(REPORT_B).into(),
        stage: ScreeningStage::FullText,
        protocol_version_id: id(PROTOCOL).into(),
        expected_revision: 0,
        title: None,
        abstract_text: None,
        document_hash: None,
        retrieval_query: None,
        criteria: full_text_protocol().iter().map(prompt).collect(),
    };
    let context = task.build_context(&input).expect("full-text context");
    let user: Value = serde_json::from_str(&context.user_prompt).expect("user prompt is JSON");
    assert_eq!(user["allowed_evidence"], json!({}));

    // A metadata citation is still invalid at full text, even if the model makes it.
    let answer = json!({
        "report_id": REPORT_B,
        "expected_revision": 0,
        "stage": "full_text",
        "protocol_version_id": PROTOCOL,
        "criteria": [
            {"criterion_id": ACTIVITY, "judgment": "meets", "rationale": "Reported.", "evidence": [metadata(REPORT_B, "abstract", REPORT_B_ABSTRACT)]},
            {"criterion_id": NOT_PRIMARY, "judgment": "does_not_meet", "rationale": "Primary.", "evidence": [metadata(REPORT_B, "abstract", REPORT_B_ABSTRACT)]},
        ],
        "suggested_decision": {"kind": "include"},
        "uncertainties": [],
    });
    let output = typed(answer).expect("typed");
    assert!(
        task.semantic_validate_with_evidence(&output, &[]).is_err(),
        "metadata citations stay invalid at full text"
    );
}

/// Scripted provider. Each call returns the next queued answer and records the
/// request it received. The handles are shared so the test can read them after
/// the gateway has been moved into the metered wrapper.
#[derive(Clone, Default)]
struct Script {
    answers: Arc<Mutex<VecDeque<Result<String, AiError>>>>,
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
}

impl Script {
    fn with(answers: Vec<Result<String, AiError>>) -> Self {
        Self {
            answers: Arc::new(Mutex::new(answers.into())),
            requests: Arc::default(),
        }
    }
}

impl AiGateway for Script {
    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        Box::pin(async move {
            self.requests.lock().expect("requests").push(request);
            let next = self
                .answers
                .lock()
                .expect("answers")
                .pop_front()
                .unwrap_or_else(|| Err(AiError::Gateway("no scripted answer".to_owned())))?;
            Ok(GatewayCompletion {
                output_json: next,
                input_tokens: 100,
                output_tokens: 50,
                cost_micros: None,
            })
        })
    }
}

#[derive(Clone, Default)]
struct Ledger(Arc<Mutex<Vec<UsageEntry>>>);

impl UsageLedger for Ledger {
    fn budget<'a>(&'a self, _project_id: ProjectId) -> AiFuture<'a, BudgetSnapshot> {
        Box::pin(async {
            Ok(BudgetSnapshot {
                budget_micros: 1_000_000_000,
                spent_micros: 0,
            })
        })
    }

    fn record<'a>(&'a self, entry: UsageEntry) -> AiFuture<'a, ()> {
        Box::pin(async move {
            self.0.lock().expect("ledger").push(entry);
            Ok(())
        })
    }
}

struct Route;

impl ModelRouter for Route {
    fn resolve<'a>(&'a self, _profile: ModelProfile) -> AiFuture<'a, ResolvedModel> {
        Box::pin(async {
            Ok(ResolvedModel {
                profile: ModelProfile::Reasoning,
                provider: "opencode-go".to_owned(),
                model: "glm-5.3-flash".to_owned(),
                model_version: "glm-5.3-flash".to_owned(),
                parameters: ModelParameters::default(),
                route_id: None,
            })
        })
    }
}

struct NoRetrieval;

impl EvidenceRetriever for NoRetrieval {
    fn retrieve<'a>(&'a self, _request: RetrievalRequest) -> AiFuture<'a, Vec<GroundedBlock>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[derive(Clone, Default)]
struct Runs(Arc<Mutex<Vec<AiRunRecord>>>);

impl AiRunStore for Runs {
    fn find_reusable<'a>(
        &'a self,
        _project_id: Option<ProjectId>,
        _reuse_hash: &'a str,
    ) -> AiFuture<'a, Option<AiRunRecord>> {
        Box::pin(async { Ok(None) })
    }

    fn save_run<'a>(&'a self, run: AiRunRecord) -> AiFuture<'a, ()> {
        Box::pin(async move {
            let mut runs = self.0.lock().expect("runs");
            runs.retain(|existing| existing.id != run.id);
            runs.push(run);
            Ok(())
        })
    }
}

#[derive(Default)]
struct Proposals(Mutex<Vec<AiProposal>>);

impl ProposalStore for Proposals {
    fn find_for_run<'a>(&'a self, run_id: Uuid) -> AiFuture<'a, Option<AiProposal>> {
        Box::pin(async move {
            Ok(self
                .0
                .lock()
                .expect("proposals")
                .iter()
                .find(|proposal| proposal.model_run_id == run_id)
                .cloned())
        })
    }

    fn create<'a>(&'a self, proposal: AiProposal) -> AiFuture<'a, AiProposal> {
        Box::pin(async move {
            self.0.lock().expect("proposals").push(proposal.clone());
            Ok(proposal)
        })
    }
}

struct RunOutcome {
    result: Result<AiTaskResult<ScreeningAnalysis>, AiError>,
    script: Script,
    ledger: Ledger,
    runs: Runs,
}

/// Runs the screening task through the real runner, with the production
/// metering wrapper in front of the scripted provider.
async fn run_screening(
    answers: Vec<Result<String, AiError>>,
    task: &ScreeningTask,
    report: &str,
) -> RunOutcome {
    let script = Script::with(answers);
    let ledger = Ledger::default();
    let gateway = MeteredGateway::new(script.clone(), Arc::new(ledger.clone()));
    let runs = Runs::default();
    let proposals = Proposals::default();
    let runner = AiTaskRunner::new(
        &gateway,
        &Route,
        &NoRetrieval,
        &runs,
        &proposals,
        &SystemClock,
        &UuidProvider,
    );
    let result = runner.run(task, ta_input(report)).await;
    RunOutcome {
        result,
        script,
        ledger,
        runs,
    }
}

#[tokio::test]
async fn a_rejected_answer_gets_one_repair_call_and_both_calls_are_counted() {
    let bad = captured_d().to_string();
    let good = {
        let mut repaired = captured_d();
        repaired["criteria"][4]["judgment"] = json!("does_not_meet");
        repaired["criteria"][4]["rationale"] = json!("Not a validity study.");
        repaired.to_string()
    };
    let task = ta_task(REPORT_D, REPORT_D_ABSTRACT, REPORT_D_ABSTRACT);
    let outcome = run_screening(vec![Ok(bad), Ok(good)], &task, REPORT_D).await;
    let result = outcome.result.expect("the repaired run completes");
    assert_eq!(result.run.status, AiRunStatus::Completed);

    let requests = outcome.script.requests.lock().expect("requests").clone();
    assert_eq!(
        requests.len(),
        2,
        "one repair call after the rejected answer"
    );
    assert!(!requests[0].user_prompt.contains("Validation rejected"));
    assert!(
        requests[1]
            .user_prompt
            .contains("Validation rejected your previous answer")
    );
    assert!(requests[1].user_prompt.contains(VALIDITY_ONLY));

    let entries = outcome.ledger.0.lock().expect("ledger").clone();
    assert_eq!(
        entries.len(),
        2,
        "the usage ledger records both provider calls"
    );
    let ledger_cost: i64 = entries.iter().map(|entry| entry.cost_micros).sum();
    assert_eq!(result.run.usage.input_tokens, 200);
    assert_eq!(result.run.usage.output_tokens, 100);
    assert_eq!(result.run.cost_micros, Some(ledger_cost));
    assert_eq!(outcome.runs.0.lock().expect("runs").len(), 1);
}

#[tokio::test]
async fn a_second_rejection_fails_the_run_with_both_calls_recorded() {
    let bad = captured_c().to_string();
    let task = ta_task(REPORT_C, REPORT_C_ABSTRACT, REPORT_C_ABSTRACT);
    let outcome = run_screening(vec![Ok(bad.clone()), Ok(bad)], &task, REPORT_C).await;
    assert!(matches!(
        outcome.result,
        Err(AiError::SchemaValidation(_) | AiError::SemanticValidation(_))
    ));
    assert_eq!(outcome.script.requests.lock().expect("requests").len(), 2);
    assert_eq!(outcome.ledger.0.lock().expect("ledger").len(), 2);
    let runs = outcome.runs.0.lock().expect("runs").clone();
    let failed = runs.last().expect("the failed run is persisted");
    assert_eq!(failed.status, AiRunStatus::Failed);
    assert_eq!(failed.usage.input_tokens, 200);
}

#[tokio::test]
async fn provider_failures_are_not_repaired() {
    let task = ta_task(REPORT_D, REPORT_D_ABSTRACT, REPORT_D_ABSTRACT);
    let outcome = run_screening(
        vec![Err(AiError::Gateway("provider request failed".to_owned()))],
        &task,
        REPORT_D,
    )
    .await;
    assert!(matches!(outcome.result, Err(AiError::Gateway(_))));
    assert_eq!(outcome.script.requests.lock().expect("requests").len(), 1);
    assert!(outcome.ledger.0.lock().expect("ledger").is_empty());
}
