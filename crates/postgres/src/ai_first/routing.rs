use super::{AiFirstError, invalidate_in_transaction, ledger, lock_project};
use deepref_domain::{Actor, ActorKind, ProjectId, ProtocolVersionId, ReportId, ScreeningStage};
use deepref_review::{
    ReviewDefinitionKey, ReviewOrigin, ReviewScheduler, ReviewSubject, ScheduleReviewRun,
    worker::PreparedReviewTask,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

pub(crate) async fn ensure_current(
    pool: &PgPool,
    tx: &mut Transaction<'_, Postgres>,
    c: &sqlx::postgres::PgRow,
) -> Result<bool, AiFirstError> {
    let project = c.get::<Uuid, _>("project_id");
    let cohort = c.get::<Uuid, _>("id");
    // Keep the identity and frozen sources stable until the approving transaction
    // commits. Metadata edits and model-route writes do not take the project lock.
    sqlx::query("LOCK TABLE ai_model_routes IN SHARE MODE")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SELECT report_id FROM ai_screening_cohort_members WHERE cohort_id=$1 FOR SHARE")
        .bind(cohort)
        .fetch_all(&mut **tx)
        .await?;
    sqlx::query("SELECT r.id FROM reports r JOIN ai_screening_cohort_members m ON m.report_id=r.id WHERE m.cohort_id=$1 FOR SHARE OF r")
        .bind(cohort).fetch_all(&mut **tx).await?;
    let report:Option<Uuid>=sqlx::query_scalar("SELECT report_id FROM ai_screening_cohort_members WHERE cohort_id=$1 ORDER BY report_id LIMIT 1")
        .bind(cohort).fetch_optional(&mut **tx).await?;
    let protocol:Option<Uuid>=sqlx::query_scalar("SELECT id FROM protocol_versions WHERE project_id=$1 AND status='published' ORDER BY version DESC LIMIT 1")
        .bind(project).fetch_optional(&mut **tx).await?;
    let same_protocol = protocol == Some(c.get("protocol_version_id"));
    let current = if let Some(report) = report {
        crate::preview_screening_identity(
            pool,
            project,
            report,
            deepref_ai::ScreeningStage::TitleAbstract,
        )
        .await
        .ok()
        .and_then(|i| i.aggregate_hash().ok())
        .is_some_and(|h| h.as_str() == c.get::<String, _>("semantic_bundle_hash"))
    } else {
        false
    };
    if !same_protocol || !current {
        invalidate_in_transaction(
            tx,
            project,
            cohort,
            if !same_protocol {
                "protocol_changed"
            } else {
                "semantic_identity_changed"
            },
            true,
        )
        .await?;
        return Ok(false);
    }
    if !super::audit::frozen_evidence_current(tx, c).await? {
        invalidate_in_transaction(tx, project, cohort, "frozen_evidence_changed", true).await?;
        return Ok(false);
    }
    Ok(true)
}

pub(crate) async fn admit_ai_first(
    tx: &mut Transaction<'_, Postgres>,
    request: &crate::PreparedReviewRun,
    hash: &str,
    cohort: Uuid,
) -> Result<(), crate::PostgresReviewError> {
    let ReviewSubject::Screening {
        report_id,
        stage: ScreeningStage::TitleAbstract,
        protocol_version_id,
        expected_revision,
    } = request.command.subject
    else {
        return Err(crate::PostgresReviewError::InvalidState(
            "AI-first is title/abstract only".into(),
        ));
    };
    let admitted: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ai_screening_cohorts c
       JOIN project_ai_screening_authority a ON a.project_id=c.project_id
       JOIN ai_screening_cohort_members m ON m.cohort_id=c.id WHERE c.project_id=$1 AND c.id=$2
       AND c.status='open' AND a.ceiling<>'off' AND a.suspended_reason IS NULL
       AND c.semantic_bundle_hash=$3 AND c.protocol_version_id=$4
       AND m.report_id=$5 AND m.evaluated_revision=$6)",
    )
    .bind(request.command.project_id.as_uuid())
    .bind(cohort)
    .bind(hash)
    .bind(protocol_version_id.as_uuid())
    .bind(report_id.as_uuid())
    .bind(expected_revision)
    .fetch_one(&mut **tx)
    .await?;
    if !admitted {
        return Err(crate::PostgresReviewError::InvalidState(
            "AI-first cohort authority or identity is stale".into(),
        ));
    }
    Ok(())
}

/// Consume only a completed compiled screening run with both validated screens.
/// A normal suggestion or a legacy run can never create a disposition.
pub async fn route_ai_first_result(
    pool: &PgPool,
    project: Uuid,
    cohort: Uuid,
    report: Uuid,
    run_id: Uuid,
) -> Result<bool, AiFirstError> {
    let mut tx = pool.begin().await?;
    lock_project(&mut tx, project).await?;
    let c =
        sqlx::query("SELECT * FROM ai_screening_cohorts WHERE project_id=$1 AND id=$2 FOR UPDATE")
            .bind(project)
            .bind(cohort)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AiFirstError::NotFound)?;
    if c.get::<String, _>("status") != "open" {
        return Ok(false);
    }
    if !ensure_current(pool, &mut tx, &c).await? {
        tx.commit().await?;
        return Ok(false);
    }
    let r=sqlx::query("SELECT r.prepared_task,r.origin,r.semantic_bundle_hash,p.payload,p.model_run_id
       FROM review_run_manifests r JOIN ai_proposals p ON p.id=r.proposal_id AND p.project_id=r.project_id
       JOIN ai_screening_cohort_members m ON m.review_run_id=r.automation_run_id AND m.project_id=r.project_id
       WHERE r.project_id=$1 AND r.automation_run_id=$2 AND r.state='completed' AND m.cohort_id=$3 AND m.report_id=$4")
        .bind(project).bind(run_id).bind(cohort).bind(report).fetch_optional(&mut *tx).await?;
    let Some(r) = r else {
        return Ok(false);
    };
    let origin: ReviewOrigin = serde_json::from_value(r.get("origin"))?;
    if origin != (ReviewOrigin::AiFirstTriggered { cohort_id: cohort })
        || r.get::<String, _>("semantic_bundle_hash") != c.get::<String, _>("semantic_bundle_hash")
    {
        return Ok(false);
    }
    let prepared: PreparedReviewTask = serde_json::from_value(r.get("prepared_task"))?;
    let PreparedReviewTask::Screening {
        input,
        criteria,
        allowed_evidence,
        ..
    } = prepared
    else {
        return Ok(false);
    };
    let state=sqlx::query("SELECT COALESCE(s.title_abstract_status,'unscreened') AS status,COALESCE(s.revision,0) AS revision
       FROM project_reports pr LEFT JOIN screening_state s ON s.project_id=pr.project_id AND s.report_id=pr.report_id
       WHERE pr.project_id=$1 AND pr.report_id=$2 FOR UPDATE OF pr")
        .bind(project).bind(report).fetch_one(&mut *tx).await?;
    if state.get::<String, _>("status") != "unscreened"
        || state.get::<i64, _>("revision") != input.expected_revision
    {
        return Ok(false);
    }
    let artifacts = sqlx::query(
        "SELECT a.node_id,artifact.payload FROM review_step_attempts a
       JOIN review_artifacts artifact ON artifact.id=a.artifact_id
       WHERE a.project_id=$1 AND a.automation_run_id=$2 AND a.accepted_at IS NOT NULL",
    )
    .bind(project)
    .bind(run_id)
    .fetch_all(&mut *tx)
    .await?;
    // Repair may alter decisive evidence; V1 sends all repaired results to humans.
    if artifacts
        .iter()
        .any(|a| a.get::<String, _>("node_id") == "semantic_repair")
    {
        return Ok(false);
    }
    let analysis = |node: &str| -> Result<Option<deepref_ai::ScreeningAnalysis>, AiFirstError> {
        artifacts
            .iter()
            .find(|a| a.get::<String, _>("node_id") == node)
            .map(|a| {
                let payload: Value = a.get("payload");
                serde_json::from_value(payload.get("output").cloned().unwrap_or(Value::Null))
                    .map_err(AiFirstError::from)
            })
            .transpose()
    };
    let (Some(primary), Some(independent)) =
        (analysis("primary_screen")?, analysis("independent_screen")?)
    else {
        return Ok(false);
    };
    if !deepref_review::automation_eligible_exclusion(&input, &criteria, &primary, &independent)
        || [&primary, &independent].iter().any(|a| {
            a.criteria
                .iter()
                .flat_map(|j| &j.evidence)
                .any(|e| !allowed_evidence.contains(e))
        })
    {
        return Ok(false);
    }
    // Reused artifacts from a repair in another run must not bypass the V1
    // no-repair rule. The final candidate must be the validated primary run.
    let primary_model_run = artifacts
        .iter()
        .find(|a| a.get::<String, _>("node_id") == "primary_screen")
        .and_then(|a| {
            a.get::<Value, _>("payload")
                .get("model_run_id")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
        });
    if primary_model_run != Some(r.get::<Uuid, _>("model_run_id")) {
        return Ok(false);
    }
    // The candidate itself must remain an exclude after the candidate audit.
    let payload: Value = r.get("payload");
    if payload
        .pointer("/suggested_decision/kind")
        .and_then(Value::as_str)
        != Some("exclude")
    {
        return Ok(false);
    }
    let changed=sqlx::query("INSERT INTO ai_screening_dispositions(project_id,cohort_id,report_id,review_run_id,ai_run_id,
       evaluated_revision,protocol_version_id,semantic_bundle_hash,policy_version,source_snapshot)
       VALUES($1,$2,$3,$4,$5,$6,$7,$8,1,$9) ON CONFLICT DO NOTHING")
        .bind(project).bind(cohort).bind(report).bind(run_id).bind(r.get::<Uuid,_>("model_run_id"))
        .bind(input.expected_revision).bind(input.protocol_version_id.as_uuid()).bind(c.get::<String,_>("semantic_bundle_hash"))
        .bind(json!({"title":input.title,"abstract_text":input.abstract_text}))
        .execute(&mut *tx).await?.rows_affected();
    if changed == 1 {
        let actor = Actor::new(ActorKind::Automation, "ai-first-routing")
            .map_err(|e| AiFirstError::Invalid(e.to_string()))?;
        // IDs/provenance only; no verdict appears in user-visible run notifications.
        ledger(
            &mut tx,
            project,
            cohort,
            "ai_first_disposition_recorded",
            json!({"policy_version":1,"scientific_state":"unscreened"}),
            &actor,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(changed == 1)
}

/// Bounded scheduling, identity monitoring and partial-budget closure on each tick.
pub async fn sweep_ai_first(pool: &PgPool) -> Result<u64, AiFirstError> {
    let cohorts=sqlx::query("SELECT id,project_id FROM ai_screening_cohorts WHERE status IN ('open','closed','auditing','passed','finalized') ORDER BY created_at")
        .fetch_all(pool).await?;
    let mut scheduled = 0u64;
    for row in cohorts {
        let project: Uuid = row.get("project_id");
        let cohort: Uuid = row.get("id");
        let mut tx = pool.begin().await?;
        lock_project(&mut tx, project).await?;
        let c = sqlx::query("SELECT * FROM ai_screening_cohorts WHERE id=$1 FOR UPDATE")
            .bind(cohort)
            .fetch_one(&mut *tx)
            .await?;
        let current = ensure_current(pool, &mut tx, &c).await?;
        tx.commit().await?;
        if !current || c.get::<String, _>("status") != "open" {
            continue;
        }
        let finished=sqlx::query("SELECT m.report_id,m.review_run_id FROM ai_screening_cohort_members m
          JOIN review_run_manifests r ON r.automation_run_id=m.review_run_id
          WHERE m.cohort_id=$1 AND r.state='completed' AND NOT EXISTS(SELECT 1 FROM ai_screening_dispositions d
            WHERE d.cohort_id=m.cohort_id AND d.report_id=m.report_id)")
            .bind(cohort).fetch_all(pool).await?;
        for r in finished {
            route_ai_first_result(
                pool,
                project,
                cohort,
                r.get("report_id"),
                r.get("review_run_id"),
            )
            .await?;
        }
        let budget = crate::get_ai_budget(pool, project)
            .await
            .map_err(|e| AiFirstError::Refused(e.to_string()))?;
        let reports=sqlx::query("SELECT m.report_id,m.evaluated_revision FROM ai_screening_cohort_members m
           LEFT JOIN screening_state s ON s.project_id=m.project_id AND s.report_id=m.report_id
           WHERE m.cohort_id=$1 AND m.review_run_id IS NULL AND COALESCE(s.title_abstract_status,'unscreened')='unscreened'
           ORDER BY sha256(convert_to('ai-first-routing-v1:' || m.cohort_id::text || ':' || m.report_id::text,'UTF8')), m.report_id LIMIT 5").bind(cohort).fetch_all(pool).await?;
        let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ai_screening_cohort_members m JOIN review_run_manifests r
           ON r.automation_run_id=m.review_run_id WHERE m.cohort_id=$1 AND r.state IN ('queued','running'))")
            .bind(cohort).fetch_one(pool).await?;
        if budget.spent_micros >= budget.budget_micros || (reports.is_empty() && !pending) {
            let actor = Actor::new(ActorKind::System, "ai-first-cohort-close")
                .map_err(|e| AiFirstError::Invalid(e.to_string()))?;
            let mut tx = pool.begin().await?;
            lock_project(&mut tx, project).await?;
            sqlx::query("UPDATE ai_screening_cohorts SET status='closed',closed_at=now() WHERE id=$1 AND status='open'")
                .bind(cohort).execute(&mut *tx).await?;
            ledger(&mut tx,project,cohort,"ai_first_cohort_closed",json!({"reason":if reports.is_empty() {"covered_corpus_complete"} else {"budget_exhausted"},"unprocessed_records":"human_queue"}),&actor).await?;
            tx.commit().await?;
            continue;
        }
        for r in reports {
            let report = r.get::<Uuid, _>("report_id");
            let actor = Actor::new(ActorKind::Automation, "ai-first-routing")
                .map_err(|e| AiFirstError::Invalid(e.to_string()))?;
            let result = crate::PostgresReviewScheduler::new(pool)
                .schedule(ScheduleReviewRun {
                    project_id: ProjectId::new(project),
                    definition: ReviewDefinitionKey::Screening,
                    subject: ReviewSubject::Screening {
                        report_id: ReportId::new(report),
                        stage: ScreeningStage::TitleAbstract,
                        protocol_version_id: ProtocolVersionId::new(c.get("protocol_version_id")),
                        expected_revision: r.get("evaluated_revision"),
                    },
                    origin: ReviewOrigin::AiFirstTriggered { cohort_id: cohort },
                    actor,
                })
                .await;
            match result {
                Ok(run) => {
                    sqlx::query("UPDATE ai_screening_cohort_members SET review_run_id=$3 WHERE cohort_id=$1 AND report_id=$2 AND review_run_id IS NULL")
                    .bind(cohort).bind(report).bind(run.id.as_uuid()).execute(pool).await?;
                    scheduled += 1;
                }
                Err(error) => {
                    tracing::warn!(%error,%project,%cohort,"AI-first scheduling refused; humans retain the record");
                    break;
                }
            }
        }
    }
    Ok(scheduled)
}
