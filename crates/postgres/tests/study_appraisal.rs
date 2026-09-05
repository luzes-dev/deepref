#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::BTreeMap;

use deepref_application::{
    AppraisalAssessmentInput, AssignReportToStudy, ClassifyStudy, CreateStudy, DefinitionId,
    DefinitionVersion, EvidenceReferenceInput, RemoveReportFromStudy, get_appraisal_definition,
};
use deepref_domain::{Actor, ActorKind, ProjectId, StudyDesign, StudyDesignContext, StudyTitle};
use deepref_postgres::{
    StudyError, assign_report_to_study, classify_study, complete_appraisal, create_study,
    get_prisma_projection, get_study, list_study_events, migrate, remove_report_from_study,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::time::{Duration, timeout};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .ok()?;
    migrate(&pool).await.ok()?;
    Some(pool)
}

async fn seed_project(pool: &PgPool, name: &str, report_ids: &[Uuid]) -> Uuid {
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,$2)")
        .bind(project_id)
        .bind(name)
        .execute(pool)
        .await
        .expect("project inserts");
    for report_id in report_ids {
        sqlx::query("INSERT INTO reports(id,title) VALUES($1,$2) ON CONFLICT (id) DO NOTHING")
            .bind(report_id)
            .bind(format!("Report {report_id}"))
            .execute(pool)
            .await
            .expect("report inserts");
        sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
            .bind(project_id)
            .bind(report_id)
            .execute(pool)
            .await
            .expect("project report inserts");
    }
    project_id
}

fn actor() -> Actor {
    Actor::new(ActorKind::User, "pr9-integration-test").expect("actor is valid")
}

#[tokio::test]
async fn study_grouping_is_reversible_and_appraisal_is_immutable_and_scoped() {
    let Some(pool) = database().await else { return };
    let report_a = Uuid::new_v4();
    let report_b = Uuid::new_v4();
    let project_a = seed_project(&pool, "PR9 grouping", &[report_a, report_b]).await;
    let project_b = seed_project(&pool, "PR9 shared report", &[report_a]).await;
    let study_a = Uuid::new_v4();
    let study_b = Uuid::new_v4();
    let study_c = Uuid::new_v4();
    let actor = actor();

    let created_a = create_study(
        &pool,
        CreateStudy {
            project_id: ProjectId::from(project_a),
            study_id: study_a.into(),
            title: StudyTitle::new("Investigation A").unwrap(),
            actor: actor.clone(),
        },
    )
    .await
    .expect("study A creates");
    let created_b = create_study(
        &pool,
        CreateStudy {
            project_id: ProjectId::from(project_a),
            study_id: study_b.into(),
            title: StudyTitle::new("Investigation B").unwrap(),
            actor: actor.clone(),
        },
    )
    .await
    .expect("study B creates");
    create_study(
        &pool,
        CreateStudy {
            project_id: ProjectId::from(project_b),
            study_id: study_c.into(),
            title: StudyTitle::new("Shared investigation").unwrap(),
            actor: actor.clone(),
        },
    )
    .await
    .expect("cross-project study creates");
    let cross_project = assign_report_to_study(
        &pool,
        AssignReportToStudy {
            project_id: project_b.into(),
            study_id: study_c.into(),
            report_id: report_a.into(),
            role: deepref_domain::StudyReportRole::ReportOfStudy,
            expected_revision: 0,
            expected_previous_study_id: None,
            expected_previous_study_revision: None,
            actor: actor.clone(),
        },
    )
    .await
    .expect("shared report assigns independently in another project");
    assert_eq!(cross_project.reports.len(), 1);

    let assigned = assign_report_to_study(
        &pool,
        AssignReportToStudy {
            project_id: project_a.into(),
            study_id: study_a.into(),
            report_id: report_a.into(),
            role: deepref_domain::StudyReportRole::ReportOfStudy,
            expected_revision: created_a.study.revision as u64,
            expected_previous_study_id: None,
            expected_previous_study_revision: None,
            actor: actor.clone(),
        },
    )
    .await
    .expect("report assigns");
    let assigned_again = assign_report_to_study(
        &pool,
        AssignReportToStudy {
            project_id: project_a.into(),
            study_id: study_a.into(),
            report_id: report_b.into(),
            role: deepref_domain::StudyReportRole::FollowUp,
            expected_revision: assigned.study.revision as u64,
            expected_previous_study_id: None,
            expected_previous_study_revision: None,
            actor: actor.clone(),
        },
    )
    .await
    .expect("second report assigns");

    classify_study(
        &pool,
        ClassifyStudy {
            project_id: project_a.into(),
            study_id: study_a.into(),
            design: StudyDesign::Rct,
            context: StudyDesignContext {
                physiotherapy: true,
                ..StudyDesignContext::default()
            },
            expected_revision: assigned_again.study.revision as u64,
            actor: actor.clone(),
        },
    )
    .await
    .expect("classification succeeds");
    let conflict = classify_study(
        &pool,
        ClassifyStudy {
            project_id: project_a.into(),
            study_id: study_a.into(),
            design: StudyDesign::Cohort,
            context: StudyDesignContext::default(),
            expected_revision: 0,
            actor: actor.clone(),
        },
    )
    .await
    .expect_err("stale classification conflicts");
    assert!(matches!(conflict, StudyError::RevisionConflict { .. }));

    let moved = assign_report_to_study(
        &pool,
        AssignReportToStudy {
            project_id: project_a.into(),
            study_id: study_b.into(),
            report_id: report_a.into(),
            role: deepref_domain::StudyReportRole::PrimaryOutcome,
            expected_revision: created_b.study.revision as u64,
            expected_previous_study_id: Some(study_a.into()),
            expected_previous_study_revision: Some(3),
            actor: actor.clone(),
        },
    )
    .await
    .expect("report moves");
    assert_eq!(moved.reports.len(), 1);
    let source_after_move = get_study(&pool, project_a, study_a).await.unwrap();
    assert!(
        source_after_move
            .reports
            .iter()
            .all(|report| report.report_id != report_a.into())
    );

    sqlx::query(
        "UPDATE study_events SET created_at = now() - interval '1 second' WHERE project_id=$1",
    )
    .bind(project_a)
    .execute(&pool)
    .await
    .expect("fixture events can be backdated deterministically");
    let prisma_before_grouping_removal = get_prisma_projection(&pool, project_a)
        .await
        .expect("PRISMA projection before grouping removal")
        .expect("grouping fixture project exists")
        .as_of
        .expect("grouping event gives PRISMA freshness");
    let unassigned = remove_report_from_study(
        &pool,
        RemoveReportFromStudy {
            project_id: project_a.into(),
            study_id: study_b.into(),
            report_id: report_a.into(),
            expected_revision: moved.study.revision as u64,
            actor: actor.clone(),
        },
    )
    .await
    .expect("report unassigns");
    assert!(unassigned.reports.is_empty());
    let prisma_after_grouping_removal = get_prisma_projection(&pool, project_a)
        .await
        .expect("PRISMA projection after grouping removal")
        .expect("grouping fixture project exists")
        .as_of
        .expect("grouping removal event gives PRISMA freshness");
    assert!(
        prisma_after_grouping_removal > prisma_before_grouping_removal,
        "append-only study event must advance PRISMA freshness after grouping removal"
    );

    let reassigned = assign_report_to_study(
        &pool,
        AssignReportToStudy {
            project_id: project_a.into(),
            study_id: study_a.into(),
            report_id: report_a.into(),
            role: deepref_domain::StudyReportRole::ReportOfStudy,
            expected_revision: source_after_move.study.revision as u64,
            expected_previous_study_id: None,
            expected_previous_study_revision: None,
            actor: actor.clone(),
        },
    )
    .await
    .expect("report reassigns");
    assert_eq!(reassigned.reports.len(), 2);

    let history = list_study_events(&pool, project_a, study_a).await.unwrap();
    assert!(
        history
            .iter()
            .any(|event| event.event_type == "report_moved")
    );
    let history_b = list_study_events(&pool, project_a, study_b).await.unwrap();
    assert!(
        history_b
            .iter()
            .any(|event| event.event_type == "report_unassigned")
    );

    let document_id = Uuid::new_v4();
    let block_id = Uuid::new_v4();
    let second_block_id = Uuid::new_v4();
    let content_hash = "a".repeat(64);
    sqlx::query(
		"INSERT INTO documents(id,project_id,report_id,source,status,object_key,content_hash,mime_type,byte_size,active_parser_version)
		 VALUES($1,$2,$3,'upload','available',$4,$5,'application/pdf',1,'parser-1')",
	)
	.bind(document_id)
	.bind(project_a)
	.bind(report_a)
	.bind(format!("documents/{document_id}"))
	.bind(content_hash)
	.execute(&pool)
	.await
	.expect("document inserts");
    sqlx::query(
        "INSERT INTO document_pages(document_id,parser_version,page_number,width,height,active)
		 VALUES($1,'parser-1',1,100,100,true)",
    )
    .bind(document_id)
    .execute(&pool)
    .await
    .expect("document page inserts");
    sqlx::query(
		"INSERT INTO document_blocks(id,document_id,parser_version,page_number,kind,section_path,ordinal,text,content_hash,active)
		 VALUES($1,$2,'parser-1',1,'text','{}',0,'Allocation was described','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',true)",
	)
	.bind(block_id)
	.bind(document_id)
	.execute(&pool)
    .await
    .expect("document block inserts");
    sqlx::query(
        "INSERT INTO document_blocks(id,document_id,parser_version,page_number,kind,section_path,ordinal,text,content_hash,active)
         VALUES($1,$2,'parser-1',1,'text','{}',1,'The allocation sequence was reported','bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',true)",
    )
    .bind(second_block_id)
    .bind(document_id)
    .execute(&pool)
    .await
    .expect("second document block inserts");

    let screening_events_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM screening_events WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_a)
    .bind(report_a)
    .fetch_one(&pool)
    .await
    .unwrap();
    let rct = get_appraisal_definition("deepref-rct-generic", 1).unwrap();
    let rct_assessment = complete_appraisal(
        &pool,
        project_a.into(),
        report_a.into(),
        AppraisalAssessmentInput {
            definition_id: DefinitionId::new("deepref-rct-generic").unwrap(),
            definition_version: DefinitionVersion::new(1).unwrap(),
            responses: serde_json::json!({
                "allocation_description": "yes",
                "outcome_measure_prespecified": true
            }),
            evidence: vec![
                EvidenceReferenceInput {
                    question_id: "allocation_description".to_owned(),
                    document_id,
                    block_id,
                    page: None,
                    parser_version: None,
                    content_hash: None,
                },
                EvidenceReferenceInput {
                    question_id: "allocation_description".to_owned(),
                    document_id,
                    block_id: second_block_id,
                    page: None,
                    parser_version: None,
                    content_hash: None,
                },
            ],
            domain_judgments: BTreeMap::from([
                ("allocation".to_owned(), "low_concern".to_owned()),
                ("outcome_reporting".to_owned(), "low_concern".to_owned()),
            ]),
            overall_judgment: Some("low_concern".to_owned()),
        },
        actor.clone(),
    )
    .await
    .expect("rct appraisal completes");
    assert_eq!(rct_assessment.definition_id, rct.id.as_str());
    assert_eq!(rct_assessment.evidence.len(), 2);
    let qualitative = get_appraisal_definition("deepref-qualitative-generic", 1).unwrap();
    let qualitative_assessment = complete_appraisal(
        &pool,
        project_a.into(),
        report_a.into(),
        AppraisalAssessmentInput {
            definition_id: qualitative.id.clone(),
            definition_version: qualitative.version,
            responses: serde_json::json!({
                "transparency_score": 2,
                "reflexivity_note": "Methods were clearly described."
            }),
            evidence: vec![EvidenceReferenceInput {
                question_id: "transparency_score".to_owned(),
                document_id,
                block_id,
                page: None,
                parser_version: None,
                content_hash: None,
            }],
            domain_judgments: BTreeMap::from([(
                "methodological_transparency".to_owned(),
                "adequate".to_owned(),
            )]),
            overall_judgment: Some("adequate".to_owned()),
        },
        actor.clone(),
    )
    .await
    .expect("qualitative appraisal completes");
    assert_eq!(
        qualitative_assessment.definition_id,
        qualitative.id.as_str()
    );
    let cross_project_error = complete_appraisal(
        &pool,
        project_b.into(),
        report_a.into(),
        AppraisalAssessmentInput {
            definition_id: DefinitionId::new("deepref-rct-generic").unwrap(),
            definition_version: DefinitionVersion::new(1).unwrap(),
            responses: serde_json::json!({
                "allocation_description": "yes",
                "outcome_measure_prespecified": true
            }),
            evidence: vec![EvidenceReferenceInput {
                question_id: "allocation_description".to_owned(),
                document_id,
                block_id,
                page: None,
                parser_version: None,
                content_hash: None,
            }],
            domain_judgments: BTreeMap::from([
                ("allocation".to_owned(), "low_concern".to_owned()),
                ("outcome_reporting".to_owned(), "low_concern".to_owned()),
            ]),
            overall_judgment: Some("low_concern".to_owned()),
        },
        actor,
    )
    .await
    .expect_err("cross-project evidence must be rejected");
    assert!(matches!(
        cross_project_error,
        deepref_postgres::AppraisalError::EvidenceNotInReport
    ));
    let screening_events_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM screening_events WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_a)
    .bind(report_a)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(screening_events_before, screening_events_after);

    let assessments: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM appraisal_assessments WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_a)
    .bind(report_a)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(assessments, 2);
    let project_b_membership: Option<Uuid> = sqlx::query_scalar(
        "SELECT study_id FROM study_reports WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_b)
    .bind(report_a)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(project_b_membership, Some(study_c));

    sqlx::query("DELETE FROM projects WHERE id = ANY($1)")
        .bind(vec![project_a, project_b])
        .execute(&pool)
        .await
        .expect("projects clean up");
}

#[tokio::test]
async fn concurrent_opposite_moves_are_ordered_and_conflict_authoritatively() {
    let Some(pool) = database().await else { return };
    let report_a = Uuid::new_v4();
    let report_b = Uuid::new_v4();
    let project_id = seed_project(&pool, "PR9 concurrent moves", &[report_a, report_b]).await;
    let study_a = Uuid::new_v4();
    let study_b = Uuid::new_v4();
    let actor = actor();

    let created_a = create_study(
        &pool,
        CreateStudy {
            project_id: project_id.into(),
            study_id: study_a.into(),
            title: StudyTitle::new("Move source A").unwrap(),
            actor: actor.clone(),
        },
    )
    .await
    .unwrap();
    let created_b = create_study(
        &pool,
        CreateStudy {
            project_id: project_id.into(),
            study_id: study_b.into(),
            title: StudyTitle::new("Move source B").unwrap(),
            actor: actor.clone(),
        },
    )
    .await
    .unwrap();
    let assigned_a = assign_report_to_study(
        &pool,
        AssignReportToStudy {
            project_id: project_id.into(),
            study_id: study_a.into(),
            report_id: report_a.into(),
            role: deepref_domain::StudyReportRole::ReportOfStudy,
            expected_revision: created_a.study.revision as u64,
            expected_previous_study_id: None,
            expected_previous_study_revision: None,
            actor: actor.clone(),
        },
    )
    .await
    .unwrap();
    let assigned_b = assign_report_to_study(
        &pool,
        AssignReportToStudy {
            project_id: project_id.into(),
            study_id: study_b.into(),
            report_id: report_b.into(),
            role: deepref_domain::StudyReportRole::ReportOfStudy,
            expected_revision: created_b.study.revision as u64,
            expected_previous_study_id: None,
            expected_previous_study_revision: None,
            actor: actor.clone(),
        },
    )
    .await
    .unwrap();

    let left_pool = pool.clone();
    let left_actor = actor.clone();
    let left = tokio::spawn(async move {
        assign_report_to_study(
            &left_pool,
            AssignReportToStudy {
                project_id: project_id.into(),
                study_id: study_b.into(),
                report_id: report_a.into(),
                role: deepref_domain::StudyReportRole::FollowUp,
                expected_revision: assigned_b.study.revision as u64,
                expected_previous_study_id: Some(study_a.into()),
                expected_previous_study_revision: Some(assigned_a.study.revision as u64),
                actor: left_actor,
            },
        )
        .await
    });
    let right_pool = pool.clone();
    let right = tokio::spawn(async move {
        assign_report_to_study(
            &right_pool,
            AssignReportToStudy {
                project_id: project_id.into(),
                study_id: study_a.into(),
                report_id: report_b.into(),
                role: deepref_domain::StudyReportRole::FollowUp,
                expected_revision: assigned_a.study.revision as u64,
                expected_previous_study_id: Some(study_b.into()),
                expected_previous_study_revision: Some(assigned_b.study.revision as u64),
                actor,
            },
        )
        .await
    });

    let joined = timeout(Duration::from_secs(5), async { tokio::join!(left, right) })
        .await
        .expect("opposite moves must not deadlock");
    let left_result = joined.0.expect("left move task must finish");
    let right_result = joined.1.expect("right move task must finish");
    let results = [left_result, right_result];
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        1,
        "exactly one opposite move should succeed: {results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(StudyError::RevisionConflict { .. })))
            .count(),
        1,
        "the losing move should report a revision conflict: {results:?}"
    );

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}
