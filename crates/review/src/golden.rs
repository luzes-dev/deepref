//! CI regression evidence for AI screening.
//!
//! Declarative asset hashes (the prompt, schema, parser and policy files) cannot
//! see a change in the Rust code that turns a screening subject into a model
//! request or a model answer into an outcome. Two fixture sections close that
//! gap by running the production code on deterministic fixtures:
//!
//! * `render` holds the canonical model requests built from the persisted
//!   screening subjects in `golden/screening-render-fixtures.json`, for every
//!   model-calling node of the screening workflow. It covers the subject
//!   mapping, the prompts, the output schema, the grounding evidence and the
//!   request body.
//! * `parse` holds the canonical interpreted outcome of every raw model
//!   answer in `golden/screening-parse-responses.txt`: the normalized analysis,
//!   the proposal payload, the independent-screen rule, the second-reviewer
//!   opinion, the workflow verdict, or the rejection.
//!
//! These fixtures are CI regression evidence and development diagnostics only:
//! they never enter production calibration identity. A compact digest of each
//! section is stored in the snapshot for quick comparison, but reviewers read
//! the sections themselves.
//!
//! The committed snapshot `golden/screening-fingerprints.json` is the code-review
//! mechanism. The `golden_snapshot_is_current` test fails when the computed report
//! differs from it, with a readable diff of the changed fixtures. When the change
//! is intended, bump `SCREENING_SEMANTIC_VERSION`, review the diff and re-bless:
//!
//! ```text
//! DEEPREF_BLESS_GOLDEN=1 cargo test -p deepref-review golden
//! ```
//!
//! The snapshot records the semantic version it was blessed at, and
//! `golden_snapshot_version_matches_the_screening_semantic_version` fails when
//! the constant and the snapshot disagree, so re-blessing without a version
//! bump fails CI. No other manual step updates the snapshot: the fixtures are
//! JSON and text, so the data is reviewable on its own; expected acceptance and
//! rejection of each response is asserted by the tests.

use std::collections::{BTreeMap, BTreeSet};

use deepref_ai::{
    AiError, AiTask, CriterionJudgment, CriterionResult, EvidenceRef, GroundedBlock,
    ModelParameters, ModelProfile, ProviderDialect, ResolvedModel, ScreeningAnalysis,
    ScreeningEvidence, ScreeningInput, ScreeningStage, ScreeningTask, SuggestedDecision,
    criteria_for_stage, hash_json, interpret_structured_response, strip_code_fence,
    structured_output_schema, structured_request_body,
};
use deepref_application::workflows::{autonomy::second_reviewer_opinion, gating::Verdict};
use deepref_domain::{
    CriterionKind, DocumentBlockId, EligibilityCriterion, ProjectId, ProtocolVersionId, ReportId,
    ScreeningStage as DomainScreeningStage,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    CompiledReviewDefinition, DefinedAiTask, ReviewCatalog, ReviewDefinitionKey, ReviewError,
    ReviewHash,
    screening_subject::{ScreeningExclusionReason, screening_task_for},
    worker::{
        PreparedReviewTask, ScreeningSubjectSource, needs_independent_screen,
        prepare_screening_task,
    },
};

const RENDER_FIXTURES: &str = include_str!("../golden/screening-render-fixtures.json");
const PARSE_RESPONSES: &str = include_str!("../golden/screening-parse-responses.txt");
#[cfg(test)]
const SNAPSHOT: &str = include_str!("../golden/screening-fingerprints.json");
#[cfg(test)]
const SNAPSHOT_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/golden/screening-fingerprints.json"
);

/// Builds the full golden result for the test harness. CI-only: production
/// calibration identity never reads this.
#[cfg(test)]
fn golden_report() -> Result<GoldenReport, ReviewError> {
    build_report()
}

/// The full golden result. The two fingerprints are digests of its `render` and
/// `parse` sections, which are also the reviewable content of the snapshot.
/// `semantic_version` records the screening semantic version the snapshot was
/// blessed at: CI requires a bump before a changed snapshot can be re-blessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct GoldenReport {
    semantic_version: u32,
    golden_render: ReviewHash,
    golden_parse: ReviewHash,
    render: Vec<RenderReport>,
    parse: Vec<ParseReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RenderReport {
    fixture: String,
    subject: ReviewHash,
    protocol: ReviewHash,
    requests: BTreeMap<&'static str, ReviewHash>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ParseReport {
    name: String,
    fixture: String,
    description: String,
    hash: ReviewHash,
}

fn build_report() -> Result<GoldenReport, ReviewError> {
    let definition = ReviewCatalog.compile(ReviewDefinitionKey::Screening)?;
    let fixtures = load_render_fixtures()?;
    let mut render = Vec::with_capacity(fixtures.len());
    for fixture in &fixtures {
        render.push(RenderReport {
            fixture: fixture.name.clone(),
            subject: ReviewHash::digest_input(&fixture.prepared)?,
            protocol: fixture.prepared.protocol_hash()?,
            requests: node_requests(&definition, fixture)?,
        });
    }
    let mut parse = Vec::new();
    for response in load_parse_responses()? {
        parse.push(parse_report(&definition, &fixtures, &response)?);
    }
    Ok(GoldenReport {
        semantic_version: crate::SCREENING_SEMANTIC_VERSION,
        golden_render: ReviewHash::digest_input(&render)?,
        golden_parse: ReviewHash::digest_input(&parse)?,
        render,
        parse,
    })
}

/// A persisted screening subject, prepared exactly as the adapter prepares it,
/// with the retrieved blocks the runner would pass as grounding evidence.
struct LoadedFixture {
    name: String,
    prepared: PreparedReviewTask,
    blocks: Vec<GroundedBlock>,
}

#[derive(Debug, Deserialize)]
struct RenderFixtureFile {
    fixtures: Vec<RenderFixtureSpec>,
}

/// One persisted screening subject as stored in the fixture file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderFixtureSpec {
    name: String,
    project_id: Uuid,
    report_id: Uuid,
    protocol_version_id: Uuid,
    stage: DomainScreeningStage,
    expected_revision: i64,
    title: Option<String>,
    #[serde(rename = "abstract")]
    abstract_text: Option<String>,
    criteria: Vec<EligibilityCriterion>,
    exclusion_reasons: Vec<ExclusionReasonSpec>,
    retrieved: Vec<RetrievedBlockSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExclusionReasonSpec {
    id: Uuid,
    stage: DomainScreeningStage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetrievedBlockSpec {
    document_block_id: Uuid,
    page: u32,
    section_path: Vec<String>,
    text: String,
    content_hash: String,
    rank: u32,
    score: f64,
}

fn load_render_fixtures() -> Result<Vec<LoadedFixture>, ReviewError> {
    let file: RenderFixtureFile = serde_json::from_str(RENDER_FIXTURES)
        .map_err(|error| fixture_error("render fixtures", error))?;
    file.fixtures.into_iter().map(load_fixture).collect()
}

fn load_fixture(spec: RenderFixtureSpec) -> Result<LoadedFixture, ReviewError> {
    let blocks = spec
        .retrieved
        .iter()
        .map(grounded_block)
        .collect::<Result<Vec<_>, _>>()?;
    let prepared = prepare_screening_task(ScreeningSubjectSource {
        project_id: ProjectId::new(spec.project_id),
        report_id: ReportId::new(spec.report_id),
        stage: spec.stage,
        protocol_version_id: ProtocolVersionId::new(spec.protocol_version_id),
        expected_revision: spec.expected_revision,
        criteria: spec.criteria,
        title: spec.title,
        abstract_text: spec.abstract_text,
        exclusion_reasons: spec
            .exclusion_reasons
            .into_iter()
            .map(|reason| ScreeningExclusionReason {
                id: reason.id,
                stage: reason.stage,
            })
            .collect(),
    })?;
    Ok(LoadedFixture {
        name: spec.name,
        prepared,
        blocks,
    })
}

fn grounded_block(spec: &RetrievedBlockSpec) -> Result<GroundedBlock, ReviewError> {
    let evidence = EvidenceRef::new(
        DocumentBlockId::new(spec.document_block_id),
        spec.page,
        spec.content_hash.clone(),
    )
    .map_err(|error| fixture_error("retrieved block", error))?
    .with_section_path(spec.section_path.clone())
    .with_retrieval(spec.rank, spec.score)
    .map_err(|error| fixture_error("retrieved block", error))?;
    let block = GroundedBlock {
        evidence,
        text: spec.text.clone(),
        retrieval_rank: spec.rank,
        retrieval_score: spec.score,
    };
    block
        .validate()
        .map_err(|error| fixture_error("retrieved block", error))?;
    Ok(block)
}

/// The model route every golden request is sent to. Fixed, so the request body
/// depends only on the code under test.
fn golden_route() -> ResolvedModel {
    ResolvedModel {
        profile: ModelProfile::Reasoning,
        provider: "golden".to_owned(),
        model: "golden-model".to_owned(),
        model_version: "golden-v1".to_owned(),
        parameters: ModelParameters {
            temperature: Some(0.0),
            max_tokens: Some(4_096),
            top_p: None,
            additional: BTreeMap::new(),
        },
        route_id: None,
    }
}

/// The request-relevant parts of a prepared screening subject.
struct SubjectParts<'a> {
    input: &'a ScreeningInput,
    criteria: &'a [EligibilityCriterion],
    evidence: &'a [ScreeningEvidence],
    reasons: &'a BTreeSet<Uuid>,
}

fn subject_parts(prepared: &PreparedReviewTask) -> Result<SubjectParts<'_>, ReviewError> {
    match prepared {
        PreparedReviewTask::Screening {
            input,
            criteria,
            allowed_evidence,
            allowed_exclusion_reasons,
        } => Ok(SubjectParts {
            input,
            criteria,
            evidence: allowed_evidence,
            reasons: allowed_exclusion_reasons,
        }),
        _ => Err(ReviewError::InvalidDefinition(
            "screening golden fixture is not a screening subject".to_owned(),
        )),
    }
}

/// The screening task bound to a workflow node, as the executor binds it.
fn bound_task(
    definition: &CompiledReviewDefinition,
    prepared: &PreparedReviewTask,
    node_id: &str,
    semantic_context: Option<Value>,
) -> Result<DefinedAiTask<ScreeningTask>, ReviewError> {
    let parts = subject_parts(prepared)?;
    let task = screening_task_for(parts.input, parts.criteria, parts.evidence, parts.reasons);
    DefinedAiTask::bind_for_node(definition.clone(), task, node_id, semantic_context)
}

/// The canonical request one node sends for one subject, rendered by the same
/// functions the runner and the gateway use.
fn render_request(
    definition: &CompiledReviewDefinition,
    fixture: &LoadedFixture,
    node_id: &str,
    semantic_context: Option<Value>,
) -> Result<Value, ReviewError> {
    let parts = subject_parts(&fixture.prepared)?;
    let task = bound_task(definition, &fixture.prepared, node_id, semantic_context)?;
    let context = task
        .build_context(parts.input)
        .map_err(|error| fixture_error("request", error))?;
    context
        .validate()
        .map_err(|error| fixture_error("request", error))?;
    let schema = structured_output_schema::<DefinedAiTask<ScreeningTask>>()
        .map_err(|error| fixture_error("request", error))?;
    Ok(structured_request_body(
        ProviderDialect::OpenCodeGo,
        &golden_route(),
        &context.system_prompt,
        &context.user_prompt,
        &fixture.blocks,
        &schema,
    ))
}

/// Digests of the requests for every model-calling node of the screening
/// workflow. The worker passes no context to the first two nodes and passes the
/// candidate (and, for repair, the audit and the protected decision) to the last
/// two; the fixed values below stand in for those.
fn node_requests(
    definition: &CompiledReviewDefinition,
    fixture: &LoadedFixture,
) -> Result<BTreeMap<&'static str, ReviewHash>, ReviewError> {
    let candidate = golden_analysis(fixture, SuggestedDecision::Include, Vec::new())?;
    let audit = golden_analysis(
        fixture,
        SuggestedDecision::Maybe,
        vec!["The golden audit questions the candidate rationale.".to_owned()],
    )?;
    let candidate_value = to_value(&candidate)?;
    let candidate_hash =
        hash_json(&candidate_value).map_err(|error| fixture_error("hash", error))?;
    let contexts: [(&'static str, Option<Value>); 4] = [
        ("primary_screen", None),
        ("independent_screen", None),
        (
            "candidate_audit",
            Some(json!({
                "candidate_hash": candidate_hash,
                "candidate": candidate_value.clone(),
            })),
        ),
        (
            "semantic_repair",
            Some(json!({
                "candidate_hash": candidate_hash,
                "candidate": candidate_value,
                "audit": to_value(&audit)?,
                "protected_decision": to_value(&candidate.suggested_decision)?,
            })),
        ),
    ];
    let mut requests = BTreeMap::new();
    for (node_id, context) in contexts {
        let body = render_request(definition, fixture, node_id, context)?;
        requests.insert(node_id, request_hash(&body)?);
    }
    Ok(requests)
}

/// A complete analysis for a fixture: every criterion the stage judges gets its
/// canonical verdict for the criterion kind, citing the grounding it may cite.
fn golden_analysis(
    fixture: &LoadedFixture,
    decision: SuggestedDecision,
    uncertainties: Vec<String>,
) -> Result<ScreeningAnalysis, ReviewError> {
    let parts = subject_parts(&fixture.prepared)?;
    let evidence = match parts.input.stage {
        ScreeningStage::TitleAbstract => parts.evidence.to_vec(),
        ScreeningStage::FullText => fixture
            .blocks
            .iter()
            .map(|block| ScreeningEvidence::DocumentBlock {
                document_block_id: block.evidence.document_block_id.as_uuid(),
                page: block.evidence.page,
                content_hash: block.evidence.content_hash.clone(),
                section_path: block.evidence.section_path.clone(),
            })
            .collect(),
    };
    let criteria = criteria_for_stage(parts.criteria, parts.input.stage)
        .into_iter()
        .map(|criterion| CriterionJudgment {
            criterion_id: criterion.id,
            judgment: match criterion.kind {
                CriterionKind::Inclusion => CriterionResult::Meets,
                CriterionKind::Exclusion => CriterionResult::DoesNotMeet,
            },
            rationale: "Golden fixture rationale.".to_owned(),
            evidence: evidence.clone(),
        })
        .collect();
    Ok(ScreeningAnalysis {
        report_id: parts.input.report_id.as_uuid(),
        expected_revision: parts.input.expected_revision,
        stage: parts.input.stage,
        protocol_version_id: parts.input.protocol_version_id.as_uuid(),
        criteria,
        suggested_decision: decision,
        uncertainties,
    })
}

/// One raw model answer and the render fixture whose subject it answers.
struct ParseResponse {
    name: String,
    fixture: String,
    raw: String,
}

/// Reads the response file: each `@@ <name> <fixture>` line starts a response
/// whose raw text is every following line.
fn load_parse_responses() -> Result<Vec<ParseResponse>, ReviewError> {
    let mut responses: Vec<ParseResponse> = Vec::new();
    for line in PARSE_RESPONSES.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            let mut parts = header.split_whitespace();
            let (Some(name), Some(fixture), None) = (parts.next(), parts.next(), parts.next())
            else {
                return Err(fixture_error("response header", line));
            };
            responses.push(ParseResponse {
                name: name.to_owned(),
                fixture: fixture.to_owned(),
                raw: String::new(),
            });
        } else if let Some(current) = responses.last_mut() {
            current.raw.push_str(line);
            current.raw.push('\n');
        } else if !line.trim().is_empty() {
            return Err(fixture_error("response", "text precedes the first header"));
        }
    }
    for response in &mut responses {
        response.raw = response.raw.trim_end_matches('\n').to_owned();
    }
    let mut names = BTreeSet::new();
    for response in &responses {
        if !names.insert(response.name.as_str()) {
            return Err(fixture_error("response", "names must be unique"));
        }
    }
    Ok(responses)
}

/// The outcome of one raw answer, as the worker would see it: the provider
/// envelope is removed by the gateway, then the runner's interpretation runs.
fn parse_report(
    definition: &CompiledReviewDefinition,
    fixtures: &[LoadedFixture],
    response: &ParseResponse,
) -> Result<ParseReport, ReviewError> {
    let fixture = fixtures
        .iter()
        .find(|fixture| fixture.name == response.fixture)
        .ok_or_else(|| {
            fixture_error("response", format!("unknown fixture {}", response.fixture))
        })?;
    let task = bound_task(definition, &fixture.prepared, "primary_screen", None)?;
    let outcome = match interpret_structured_response(
        &task,
        strip_code_fence(&response.raw),
        &fixture.blocks,
    ) {
        Ok((_, output)) => {
            let eligible = match &fixture.prepared {
                PreparedReviewTask::Screening {
                    input, criteria, ..
                } => crate::automation_eligible_exclusion(input, criteria, &output, &output),
                _ => false,
            };
            let mut outcome = accepted_outcome(&task, output)?;
            outcome.value["automation_eligible_exclusion"] = Value::Bool(eligible);
            outcome.value["automation_policy_version"] = json!(crate::AI_FIRST_POLICY_VERSION);
            outcome
        }
        Err(error) => rejected_outcome(&error),
    };
    Ok(ParseReport {
        name: response.name.clone(),
        fixture: response.fixture.clone(),
        description: outcome.description,
        hash: ReviewHash::digest_input(&outcome.value)?,
    })
}

/// A canonical outcome and the one-line description the snapshot shows.
struct Outcome {
    value: Value,
    description: String,
}

/// What an accepted answer does downstream: the analysis as stored, the proposal
/// payload that is persisted, the independent-screen rule, the second-reviewer
/// opinion, and the workflow verdict.
fn accepted_outcome(
    task: &DefinedAiTask<ScreeningTask>,
    output: ScreeningAnalysis,
) -> Result<Outcome, ReviewError> {
    let needs_independent = needs_independent_screen(&output);
    let mut proposal = task.proposal(&output).ok_or_else(|| {
        ReviewError::Execution("golden screening task did not assemble a proposal".to_owned())
    })?;
    proposal
        .payload
        .as_object_mut()
        .ok_or_else(|| {
            ReviewError::Execution("golden proposal payload must be an object".to_owned())
        })?
        .insert(
            "task_kind".to_owned(),
            Value::String(task.kind().as_str().to_owned()),
        );
    let analysis = to_value(&output)?;
    let kind = analysis
        .pointer("/suggested_decision/kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let reason = analysis
        .pointer("/suggested_decision/exclusion_reason_id")
        .cloned()
        .unwrap_or(Value::Null);
    let judgments = analysis
        .get("criteria")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.get("judgment").and_then(Value::as_str).unwrap_or("?"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let opinion = second_reviewer_opinion(&kind);
    let verdict = Verdict::from_answer(&proposal.payload);
    let verdict_name = verdict.decision.as_str();
    let description = format!(
        "accepted decision={kind} reason={reason} judgments=[{judgments}] independent={needs_independent} second_reviewer={} verdict={verdict_name}",
        opinion.unwrap_or("none"),
    );
    Ok(Outcome {
        value: json!({
            "status": "accepted",
            "analysis": analysis,
            "proposal": proposal.payload,
            "needs_independent_screen": needs_independent,
            "second_reviewer_opinion": opinion,
            "verdict": verdict,
        }),
        description,
    })
}

fn rejected_outcome(error: &AiError) -> Outcome {
    let text = format!("{error:?}");
    Outcome {
        value: json!({"status": "rejected", "error": text}),
        description: format!("rejected {text}"),
    }
}

fn request_hash(body: &Value) -> Result<ReviewHash, ReviewError> {
    ReviewHash::parse(hash_json(body).map_err(|error| fixture_error("request hash", error))?)
}

fn to_value<T: Serialize>(value: &T) -> Result<Value, ReviewError> {
    serde_json::to_value(value).map_err(|error| fixture_error("canonical value", error))
}

fn fixture_error(context: &str, error: impl std::fmt::Debug) -> ReviewError {
    ReviewError::InvalidDefinition(format!("screening golden {context} is invalid: {error:?}"))
}

#[cfg(test)]
fn snapshot_text(report: &GoldenReport) -> Result<String, ReviewError> {
    serde_json::to_string_pretty(report)
        .map(|text| format!("{text}\n"))
        .map_err(|error| fixture_error("snapshot", error))
}

#[cfg(test)]
mod tests {
    use deepref_ai::sha256_bytes;
    use deepref_domain::CriterionStage;

    use super::*;

    fn fixture_named<'a>(fixtures: &'a [LoadedFixture], name: &str) -> &'a LoadedFixture {
        fixtures
            .iter()
            .find(|fixture| fixture.name == name)
            .expect("fixture exists")
    }

    fn outcome_for_raw(fixture_name: &str, raw: &str) -> ParseReport {
        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition compiles");
        let fixtures = load_render_fixtures().expect("fixtures load");
        parse_report(
            &definition,
            &fixtures,
            &ParseResponse {
                name: "inline".to_owned(),
                fixture: fixture_name.to_owned(),
                raw: raw.to_owned(),
            },
        )
        .expect("parse report builds")
    }

    fn response_raw(name: &str) -> String {
        load_parse_responses()
            .expect("responses load")
            .into_iter()
            .find(|response| response.name == name)
            .expect("response exists")
            .raw
    }

    #[test]
    fn ai_first_requires_complete_grounded_agreement() {
        let fixtures = load_render_fixtures().expect("fixtures load");
        let fixture = fixture_named(&fixtures, "ta_full");
        let parts = subject_parts(&fixture.prepared).expect("screening parts");
        let mut analysis = golden_analysis(
            fixture,
            SuggestedDecision::Exclude {
                exclusion_reason_id: None,
            },
            vec![],
        )
        .expect("analysis");
        for judgment in &mut analysis.criteria {
            judgment.judgment = CriterionResult::DoesNotMeet;
        }
        let eligible = |input: &ScreeningInput, a: &ScreeningAnalysis, b: &ScreeningAnalysis| {
            crate::automation_eligible_exclusion(input, parts.criteria, a, b)
        };
        assert!(eligible(parts.input, &analysis, &analysis));
        let mut changed = analysis.clone();
        changed.criteria[1].evidence.clear();
        assert!(
            !eligible(parts.input, &changed, &analysis),
            "every decisive judgment needs evidence"
        );
        changed = analysis.clone();
        changed.uncertainties.push("Unclear population".into());
        assert!(!eligible(parts.input, &analysis, &changed));
        changed = analysis.clone();
        changed.suggested_decision = SuggestedDecision::Include;
        assert!(!eligible(parts.input, &analysis, &changed));
        let mut input = parts.input.clone();
        input.abstract_text = None;
        assert!(!eligible(&input, &analysis, &analysis));
        input.abstract_text = Some("Short abstract".into());
        assert!(!eligible(&input, &analysis, &analysis));
        input = parts.input.clone();
        input.stage = ScreeningStage::FullText;
        assert!(!eligible(&input, &analysis, &analysis));
        changed = analysis.clone();
        changed.criteria[0].evidence = vec![ScreeningEvidence::ReportMetadata {
            report_id: Uuid::new_v4(),
            field: deepref_ai::ScreeningEvidenceField::Title,
            content_hash: "unrelated".into(),
        }];
        assert!(!eligible(parts.input, &changed, &analysis));
    }

    #[test]
    fn golden_snapshot_version_matches_the_screening_semantic_version() {
        let snapshot: Value = serde_json::from_str(SNAPSHOT).expect("committed snapshot parses");
        assert_eq!(
            snapshot.get("semantic_version").and_then(Value::as_u64),
            Some(u64::from(crate::SCREENING_SEMANTIC_VERSION)),
            "re-blessing a changed snapshot without bumping \
             SCREENING_SEMANTIC_VERSION fails here"
        );
    }

    #[test]
    fn golden_snapshot_is_current() {
        let report = golden_report().expect("golden fixtures compute");
        let text = snapshot_text(&report).expect("snapshot serializes");
        if std::env::var("DEEPREF_BLESS_GOLDEN").as_deref() == Ok("1") {
            std::fs::write(SNAPSHOT_PATH, &text).expect("golden snapshot is writable");
            return;
        }
        if text != SNAPSHOT {
            panic!(
                "Review behavior changed.\n\
                 \n\
                 If this is intentional:\n\
                 1. inspect the semantic diff below,\n\
                 2. bump SCREENING_SEMANTIC_VERSION in crates/review/src/contract.rs,\n\
                 3. update the reviewed semantic snapshot with \
                 DEEPREF_BLESS_GOLDEN=1 cargo test -p deepref-review golden.\n\
                 \n\
                 If this is unintended:\n\
                 restore the previous behavior.\n\
                 \n\
                 {}\n\
                 (first snapshot line difference at line {})",
                describe_golden_diff(&report),
                text.lines()
                    .zip(SNAPSHOT.lines())
                    .position(|(computed, committed)| computed != committed)
                    .map_or(0, |index| index + 1),
            );
        }
    }

    /// A readable summary of what changed between the computed report and the
    /// committed snapshot: fixture names and outcome descriptions, not hashes.
    fn describe_golden_diff(report: &GoldenReport) -> String {
        const MAX_LINES: usize = 40;
        let mut lines = Vec::new();
        let committed: Value = match serde_json::from_str(SNAPSHOT) {
            Ok(value) => value,
            Err(_) => return "the committed snapshot is not valid JSON".to_owned(),
        };
        let computed = serde_json::to_value(report).unwrap_or(Value::Null);
        let committed_version = committed.get("semantic_version");
        let computed_version = computed.get("semantic_version");
        if committed_version != computed_version {
            lines.push(format!(
                "semantic_version: {committed_version:?} -> {computed_version:?}"
            ));
        }
        if committed.get("golden_render") != computed.get("golden_render") {
            diff_sections(
                &committed,
                &computed,
                "render",
                "fixture",
                &mut lines,
                render_entry_diff,
            );
        }
        if committed.get("golden_parse") != computed.get("golden_parse") {
            diff_sections(
                &committed,
                &computed,
                "parse",
                "name",
                &mut lines,
                parse_entry_diff,
            );
        }
        if lines.is_empty() {
            lines.push("the snapshot text changed without a semantic change".to_owned());
        }
        lines.truncate(MAX_LINES);
        if lines.len() == MAX_LINES {
            lines.push("... truncated".to_owned());
        }
        lines.join("\n")
    }

    /// Diffs one snapshot section (`render` or `parse`) entry by entry, keyed
    /// by `key` (`fixture` or `name`), describing each change with `describe`.
    fn diff_sections(
        committed: &Value,
        computed: &Value,
        section: &str,
        key: &str,
        lines: &mut Vec<String>,
        describe: impl Fn(&str, &Value, &Value, &mut Vec<String>),
    ) {
        fn by_key<'v>(section: &'v Value, key: &str) -> BTreeMap<String, &'v Value> {
            section
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| {
                    entry
                        .get(key)
                        .and_then(Value::as_str)
                        .map(|name| (name.to_owned(), entry))
                })
                .collect()
        }
        let before = by_key(&committed[section], key);
        let after = by_key(&computed[section], key);
        for name in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
            match (before.get(name), after.get(name)) {
                (Some(old), Some(new)) if old != new => describe(name, old, new, lines),
                (None, _) => lines.push(format!("{section} entry added: {name}")),
                (_, None) => lines.push(format!("{section} entry removed: {name}")),
                _ => {}
            }
        }
    }

    /// Names the nodes whose rendered request changed for one render fixture.
    fn render_entry_diff(name: &str, old: &Value, new: &Value, lines: &mut Vec<String>) {
        if old.get("subject") != new.get("subject") || old.get("protocol") != new.get("protocol") {
            lines.push(format!(
                "render fixture {name}: subject or protocol changed"
            ));
        }
        let requests = |entry: &Value| {
            entry
                .get("requests")
                .and_then(Value::as_object)
                .map(|object| {
                    object
                        .iter()
                        .map(|(node, hash)| (node.clone(), hash.clone()))
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default()
        };
        let (before, after) = (requests(old), requests(new));
        for node in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
            if before.get(node) != after.get(node) {
                lines.push(format!(
                    "render fixture {name}: node {node} request changed"
                ));
            }
        }
    }

    /// Shows the before/after outcome description of one changed parse response.
    fn parse_entry_diff(name: &str, old: &Value, new: &Value, lines: &mut Vec<String>) {
        let description = |entry: &Value| {
            entry
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("(no description)")
                .to_owned()
        };
        lines.push(format!(
            "parse response {name} (fixture {}):\n  - {}\n  + {}",
            new.get("fixture")
                .and_then(Value::as_str)
                .unwrap_or("(unknown fixture)"),
            description(old),
            description(new),
        ));
    }

    #[test]
    fn section_digests_match_the_report_sections() {
        let report = golden_report().expect("golden fixtures compute");
        assert_eq!(
            ReviewHash::digest_input(&report.render).expect("render digests"),
            report.golden_render
        );
        assert_eq!(
            ReviewHash::digest_input(&report.parse).expect("parse digests"),
            report.golden_parse
        );
    }

    #[test]
    fn the_golden_result_is_deterministic() {
        let first = build_report().expect("first build");
        let second = build_report().expect("second build");
        assert_eq!(first, second);
    }

    #[test]
    fn render_fixtures_cover_the_required_subjects() {
        let fixtures = load_render_fixtures().expect("fixtures load");
        assert!(fixtures.len() >= 6, "at least six render fixtures");
        let names = fixtures
            .iter()
            .map(|fixture| fixture.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), fixtures.len(), "fixture names are unique");
        for fixture in &fixtures {
            for block in &fixture.blocks {
                assert_eq!(
                    sha256_bytes(block.text.as_bytes()),
                    block.evidence.content_hash,
                    "{}: retrieved block hash must be the hash of its text",
                    fixture.name
                );
            }
        }
        let full_text = fixture_named(&fixtures, "ft_all_stages");
        let parts = subject_parts(&full_text.prepared).expect("screening subject");
        let stages = parts
            .criteria
            .iter()
            .map(|criterion| criterion.stage)
            .collect::<Vec<_>>();
        for stage in [
            CriterionStage::TitleAbstract,
            CriterionStage::FullText,
            CriterionStage::Both,
        ] {
            assert!(
                stages.contains(&stage),
                "full text fixture spans all three criterion stages: missing {stage:?}"
            );
        }
        assert!(
            parts.reasons.len() >= 2,
            "full text fixture has several exclusion reasons"
        );
    }

    #[test]
    fn every_response_is_classified_and_the_classes_hold() {
        const ACCEPTED: &[&str] = &[
            "ta_valid_include",
            "ta_valid_exclude_null_reason",
            "ta_exclude_reason_normalised_to_null",
            "ta_maybe",
            "ta_insufficient_with_uncertainty",
            "ta_aliases_and_typo",
            "ta_not_applicable_marker",
            "ta_duplicate_identical",
            "ta_wrong_order",
            "ta_extra_full_text_criterion",
            "ta_maybe_all_criteria_satisfied",
            "ta_code_fenced_json",
            "ta_decision_kind_odd_casing",
            "ta_unknown_top_level_field",
            "ft_valid_exclude_allowed_reason",
            "ft_include_block_hash_filled",
        ];
        const REJECTED: &[&str] = &[
            "ta_insufficient_without_uncertainty",
            "ta_unknown_judgment_value",
            "ta_duplicate_conflicting",
            "ta_missing_judgment",
            "ta_wrong_stage",
            "ta_wrong_report",
            "ta_wrong_revision",
            "ta_metadata_hash_not_allowed",
            "ta_include_with_exclusion_support",
            "ta_non_json_text",
            "ta_unknown_decision_kind",
            "ft_exclude_reason_wrong_stage",
            "ft_exclude_without_reason",
            "ft_metadata_evidence_at_full_text",
            "ft_document_block_not_retrieved",
            "ft_document_block_wrong_hash",
        ];
        let report = golden_report().expect("golden fixtures compute");
        let names = report
            .parse
            .iter()
            .map(|parse| parse.name.as_str())
            .collect::<BTreeSet<_>>();
        let classified = ACCEPTED
            .iter()
            .chain(REJECTED)
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names, classified,
            "every response must be classified as accepted or rejected"
        );
        for parse in &report.parse {
            let accepted = ACCEPTED.contains(&parse.name.as_str());
            assert_eq!(
                parse.description.starts_with("accepted "),
                accepted,
                "{}: {}",
                parse.name,
                parse.description
            );
        }
    }

    #[test]
    fn aliases_normalize_to_the_canonical_outcome_but_unknown_words_do_not() {
        let canonical = response_raw("ta_valid_include");
        let aliased = canonical
            .replace("\"judgment\": \"meets\"", "\"judgment\": \"met\"")
            .replace(
                "\"judgment\": \"does_not_meet\"",
                "\"judgment\": \"not_met\"",
            );
        assert_ne!(
            aliased, canonical,
            "the alias variant differs from the canonical text"
        );
        let canonical_outcome = outcome_for_raw("ta_full", &canonical);
        let aliased_outcome = outcome_for_raw("ta_full", &aliased);
        assert_eq!(aliased_outcome.hash, canonical_outcome.hash);
        assert_eq!(aliased_outcome.description, canonical_outcome.description);

        let unknown = canonical.replace("\"judgment\": \"meets\"", "\"judgment\": \"affirmed\"");
        let unknown_outcome = outcome_for_raw("ta_full", &unknown);
        assert_ne!(
            unknown_outcome.hash, canonical_outcome.hash,
            "a word the normalizer does not know changes the parse fingerprint"
        );
        assert!(unknown_outcome.description.starts_with("rejected "));
    }

    #[test]
    fn criteria_order_is_part_of_the_render_fingerprint() {
        let specs = {
            let file: RenderFixtureFile =
                serde_json::from_str(RENDER_FIXTURES).expect("fixtures parse");
            file.fixtures
        };
        let original_spec = specs
            .iter()
            .find(|spec| spec.name == "ta_full")
            .expect("ta_full exists")
            .clone();
        let mut reordered_spec = original_spec.clone();
        let first = reordered_spec.criteria[0].ordinal;
        reordered_spec.criteria[0].ordinal = reordered_spec.criteria[1].ordinal;
        reordered_spec.criteria[1].ordinal = first;

        let definition = ReviewCatalog
            .compile(ReviewDefinitionKey::Screening)
            .expect("definition compiles");
        let original = load_fixture(original_spec).expect("original loads");
        let reordered = load_fixture(reordered_spec).expect("reordered loads");
        let before = node_requests(&definition, &original).expect("requests render");
        let after = node_requests(&definition, &reordered).expect("requests render");
        assert_eq!(before.len(), 4, "every model-calling node is covered");
        for (node, hash) in &before {
            assert_ne!(
                after.get(node),
                Some(hash),
                "{node}: a different criteria order is a different request"
            );
        }
    }

    #[test]
    fn the_system_prompt_is_part_of_the_request_fingerprint() {
        let schema =
            structured_output_schema::<DefinedAiTask<ScreeningTask>>().expect("schema renders");
        let render = |prompt: &str| {
            request_hash(&structured_request_body(
                ProviderDialect::OpenCodeGo,
                &golden_route(),
                prompt,
                "{\"input\":{}}",
                &[],
                &schema,
            ))
            .expect("hash builds")
        };
        assert_ne!(
            render("Assess every applicable criterion."),
            render("Assess every applicable criterion, briefly.")
        );
    }

    #[test]
    fn request_and_outcome_hashes_ignore_json_object_order() {
        // hash_json canonicalizes objects through sorted keys, so key order in
        // a Value — and the serde_json map backend selected by Cargo features
        // such as preserve_order — cannot change reuse or fingerprint hashes.
        // Arrays stay ordered: evidence and criteria order is semantic.
        let inserted = json!({"b": 1, "a": 2, "nested": {"y": true, "x": [3, 2, 1]}});
        let sorted = json!({"a": 2, "b": 1, "nested": {"x": [3, 2, 1], "y": true}});
        assert_eq!(hash_json(&inserted), hash_json(&sorted));
        let reordered = json!({"nested": {"x": [3, 2, 1], "y": true}, "a": 2, "b": 1});
        assert_eq!(hash_json(&reordered), hash_json(&sorted));
        let different_order = json!({"a": 2, "b": 1, "nested": {"x": [2, 3, 1], "y": true}});
        assert_ne!(hash_json(&different_order), hash_json(&sorted));
    }
}
