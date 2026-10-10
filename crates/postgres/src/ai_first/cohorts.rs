use super::AiFirstError;
use deepref_domain::{Actor, ActorKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiFirstForecast {
    pub human_records_remaining: u64,
    pub minimum_sample: u32,
    pub minimum_controls: u32,
    pub human_judgments_if_passed: u64,
    pub human_judgments_if_failed: u64,
    pub savings_vs_single: i64,
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiFirstCohort {
    pub id: Uuid,
    pub status: String,
    pub target_percent: i32,
    pub members: i64,
    pub evaluated: i64,
    pub quarantined: i64,
    pub sampled: i64,
    pub controls: i64,
    pub labels: i64,
    pub reference_relevant: Option<i32>,
    pub alpha_billionths: Option<i32>,
    pub result: Option<Value>,
    pub forecast: Option<AiFirstForecast>,
    pub invalidation_reason: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiFirstOverview {
    pub ceiling: String,
    pub suspended_reason: Option<String>,
    pub cohorts: Vec<AiFirstCohort>,
}

pub(crate) async fn lock_project(
    tx: &mut Transaction<'_, Postgres>,
    project: Uuid,
) -> Result<(), AiFirstError> {
    let found: Option<Uuid> = sqlx::query_scalar("SELECT id FROM projects WHERE id=$1 FOR UPDATE")
        .bind(project)
        .fetch_optional(&mut **tx)
        .await?;
    found.ok_or(AiFirstError::NotFound)?;
    Ok(())
}
pub(crate) fn user(actor: &Actor) -> Result<(), AiFirstError> {
    if actor.kind() != ActorKind::User {
        return Err(AiFirstError::Invalid(
            "a human actor must approve this action".into(),
        ));
    }
    Ok(())
}
pub(crate) async fn ledger(
    tx: &mut Transaction<'_, Postgres>,
    project: Uuid,
    cohort: Uuid,
    event: &str,
    payload: Value,
    actor: &Actor,
) -> Result<(), AiFirstError> {
    sqlx::query("INSERT INTO review_events (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
      VALUES ($1,$2,$3,'ai_screening_cohort',$4,$5,$6,$7)")
        .bind(Uuid::new_v4()).bind(project).bind(event).bind(cohort).bind(payload)
        .bind(actor.kind().as_str()).bind(actor.id()).execute(&mut **tx).await?;
    Ok(())
}

pub async fn get_ai_first_overview(
    pool: &PgPool,
    project: Uuid,
) -> Result<AiFirstOverview, AiFirstError> {
    let setting = sqlx::query(
        "SELECT ceiling,suspended_reason FROM project_ai_screening_authority WHERE project_id=$1",
    )
    .bind(project)
    .fetch_optional(pool)
    .await?;
    let rows = sqlx::query("SELECT c.id,c.status,c.target_percent,c.reference_relevant,c.alpha_billionths,c.result,c.invalidation_reason,
      (SELECT count(*) FROM ai_screening_cohort_members m WHERE m.cohort_id=c.id) AS members,
      (SELECT count(*) FROM ai_screening_cohort_members m JOIN review_run_manifests r ON r.project_id=m.project_id AND r.automation_run_id=m.review_run_id WHERE m.cohort_id=c.id AND r.state='completed') AS evaluated,
      (SELECT count(*) FROM ai_screening_dispositions d WHERE d.cohort_id=c.id AND d.voided_at IS NULL AND d.finalized_at IS NULL) AS quarantined,
      (SELECT count(*) FROM ai_screening_cohort_members m WHERE m.cohort_id=c.id AND m.sampled) AS sampled,
      (SELECT count(*) FROM ai_screening_cohort_members m WHERE m.cohort_id=c.id AND m.audit_control) AS controls,
      (SELECT count(*) FROM ai_screening_audit_labels l WHERE l.cohort_id=c.id) AS labels
      FROM ai_screening_cohorts c WHERE c.project_id=$1 ORDER BY c.created_at DESC,c.id DESC")
        .bind(project).fetch_all(pool).await?;
    let mut cohorts = Vec::new();
    for r in &rows {
        let cohort: Uuid = r.get("id");
        let forecast = if r.get::<String, _>("status") == "closed" {
            let counts=sqlx::query("SELECT count(*) FILTER(WHERE COALESCE(s.title_abstract_status,'unscreened')='unscreened') AS remaining,
               count(*) FILTER(WHERE s.title_abstract_status IN ('include','maybe')) AS retained FROM ai_screening_cohort_members m
               LEFT JOIN screening_state s ON s.project_id=m.project_id AND s.report_id=m.report_id
               WHERE m.cohort_id=$1 AND NOT EXISTS(SELECT 1 FROM ai_screening_dispositions d WHERE d.cohort_id=m.cohort_id
                 AND d.report_id=m.report_id AND d.voided_at IS NULL AND d.finalized_at IS NULL)")
                .bind(cohort).fetch_one(pool).await?;
            let ordinal: i32 = sqlx::query_scalar(
                "SELECT next_audit_ordinal FROM project_ai_screening_authority WHERE project_id=$1",
            )
            .bind(project)
            .fetch_one(pool)
            .await?;
            if ordinal <= 25 {
                let population = r.get::<i64, _>("quarantined") as u32;
                let d = deepref_application::ai_first::AuditDesign {
                    population,
                    reference_relevant: counts.get::<i64, _>("retained") as u32,
                    sample_size: 0,
                    target_percent: r.get::<i32, _>("target_percent") as u32,
                    alpha_billionths: 50_000_000u32 >> ordinal,
                };
                let n = d
                    .zero_miss_sample()
                    .map_err(|e| AiFirstError::Refused(e.to_string()))?;
                let members = r.get::<i64, _>("members");
                let nh = members - i64::from(population);
                let controls = i64::from(n).min(nh);
                let pass = nh + 2 * (i64::from(n) + controls);
                let fail = members + i64::from(n) + 2 * controls;
                let savings = members - pass;
                Some(AiFirstForecast {
                    human_records_remaining: counts.get::<i64, _>("remaining") as u64,
                    minimum_sample: n,
                    minimum_controls: controls as u32,
                    human_judgments_if_passed: pass as u64,
                    human_judgments_if_failed: fail as u64,
                    savings_vs_single: savings,
                    recommended: savings >= 500 && savings * 5 >= members,
                })
            } else {
                None
            }
        } else {
            None
        };
        cohorts.push(AiFirstCohort {
            id: r.get("id"),
            status: r.get("status"),
            target_percent: r.get("target_percent"),
            members: r.get("members"),
            evaluated: r.get("evaluated"),
            quarantined: r.get("quarantined"),
            sampled: r.get("sampled"),
            controls: r.get("controls"),
            labels: r.get("labels"),
            reference_relevant: r.get("reference_relevant"),
            alpha_billionths: r.get("alpha_billionths"),
            result: r.get("result"),
            invalidation_reason: r.get("invalidation_reason"),
            forecast,
        });
    }
    Ok(AiFirstOverview {
        ceiling: setting.as_ref().map_or("off".into(), |r| r.get("ceiling")),
        suspended_reason: setting.as_ref().and_then(|r| r.get("suspended_reason")),
        cohorts,
    })
}

/// Freeze the eligible corpus before any AI-first work. This owner attestation
/// permits routing only; it is never an approval to exclude a single record.
pub async fn start_ai_first_cohort(
    pool: &PgPool,
    project: Uuid,
    target: u32,
    allow_finalization: bool,
    actor: &Actor,
) -> Result<Uuid, AiFirstError> {
    user(actor)?;
    if !matches!(target, 95 | 98) {
        return Err(AiFirstError::Invalid("target must be 95 or 98".into()));
    }
    let report: Uuid = sqlx::query_scalar("SELECT pr.report_id FROM project_reports pr LEFT JOIN screening_state s
      ON s.project_id=pr.project_id AND s.report_id=pr.report_id WHERE pr.project_id=$1
      AND COALESCE(s.title_abstract_status,'unscreened')='unscreened' ORDER BY pr.report_id LIMIT 1")
        .bind(project).fetch_optional(pool).await?.ok_or_else(|| AiFirstError::Refused("no unscreened records".into()))?;
    let identity = crate::preview_screening_identity(
        pool,
        project,
        report,
        deepref_ai::ScreeningStage::TitleAbstract,
    )
    .await?;
    let contract_id = identity.id();
    let protocol = crate::get_published_protocol(pool, project)
        .await
        .map_err(|e| AiFirstError::Refused(e.to_string()))?;
    let mut tx = pool.begin().await?;
    lock_project(&mut tx, project).await?;
    let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ai_screening_cohorts WHERE project_id=$1 AND status IN ('open','closed','auditing','passed'))")
        .bind(project).fetch_one(&mut *tx).await?;
    if active {
        return Err(AiFirstError::Refused(
            "finish or recover the current cohort first".into(),
        ));
    }
    let ceiling = if allow_finalization {
        "cohort_finalization"
    } else {
        "routing"
    };
    sqlx::query("INSERT INTO project_ai_screening_authority(project_id,ceiling,approved_by) VALUES($1,$2,$3)
       ON CONFLICT(project_id) DO UPDATE SET ceiling=$2,approved_by=$3,suspended_reason=NULL,updated_at=now()")
        .bind(project).bind(ceiling).bind(actor.id()).execute(&mut *tx).await?;
    let published:Option<Uuid>=sqlx::query_scalar("SELECT id FROM protocol_versions WHERE project_id=$1 AND status='published' ORDER BY version DESC LIMIT 1")
        .bind(project).fetch_optional(&mut *tx).await?;
    if published != Some(protocol.id) {
        return Err(AiFirstError::Refused(
            "protocol changed while preparing cohort".into(),
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO ai_screening_cohorts(id,project_id,protocol_version_id,semantic_bundle_hash,semantic_identity,policy_version,target_percent,approved_by)
      VALUES($1,$2,$3,$4,$5,1,$6,$7)")
        .bind(id).bind(project).bind(protocol.id).bind(contract_id.as_str()).bind(serde_json::to_value(&identity)?)
        .bind(target as i32).bind(actor.id()).execute(&mut *tx).await?;
    let members = sqlx::query(
        "INSERT INTO ai_screening_cohort_members(project_id,cohort_id,report_id,evaluated_revision)
      SELECT pr.project_id,$2,pr.report_id,COALESCE(s.revision,0) FROM project_reports pr
      LEFT JOIN screening_state s ON s.project_id=pr.project_id AND s.report_id=pr.report_id
      WHERE pr.project_id=$1 AND COALESCE(s.title_abstract_status,'unscreened')='unscreened'
      AND NOT EXISTS(SELECT 1 FROM ai_screening_cohort_members prior JOIN ai_screening_cohorts old ON old.id=prior.cohort_id
        WHERE prior.project_id=pr.project_id AND prior.report_id=pr.report_id AND old.sampling_seed IS NOT NULL
        AND old.status IN ('failed','invalidated'))",
    )
    .bind(project)
    .bind(id)
    .execute(&mut *tx)
    .await?.rows_affected();
    if members == 0 {
        return Err(AiFirstError::Refused("no fresh records remain; previously audited failed or invalidated frames require human screening".into()));
    }
    ledger(
        &mut tx,
        project,
        id,
        "ai_first_routing_approved",
        json!({"ceiling":ceiling,"protocol_version_id":protocol.id,
       "semantic_identity":identity,"target_percent":target,"scientific_authority":"none"}),
        actor,
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn close_ai_first_cohort(
    pool: &PgPool,
    project: Uuid,
    cohort: Uuid,
    actor: &Actor,
) -> Result<(), AiFirstError> {
    user(actor)?;
    let mut tx = pool.begin().await?;
    lock_project(&mut tx, project).await?;
    let changed=sqlx::query("UPDATE ai_screening_cohorts SET status='closed',closed_at=now() WHERE project_id=$1 AND id=$2 AND status='open'")
        .bind(project).bind(cohort).execute(&mut *tx).await?.rows_affected();
    if changed != 1 {
        return Err(AiFirstError::Refused("cohort is not open".into()));
    }
    ledger(
        &mut tx,
        project,
        cohort,
        "ai_first_cohort_closed",
        json!({"reason":"owner"}),
        actor,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn invalidate_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    project: Uuid,
    cohort: Uuid,
    reason: &str,
    suspend: bool,
) -> Result<(), AiFirstError> {
    reopen_finalized(tx, project, cohort, reason).await?;
    sqlx::query("UPDATE ai_screening_cohorts SET status='invalidated',invalidation_reason=$3 WHERE project_id=$1 AND id=$2 AND status IN ('open','closed','auditing','passed','finalized')")
        .bind(project).bind(cohort).bind(reason).execute(&mut **tx).await?;
    sqlx::query("UPDATE ai_screening_dispositions SET voided_at=clock_timestamp(),void_reason=$3 WHERE project_id=$1 AND cohort_id=$2 AND voided_at IS NULL AND finalized_at IS NULL")
        .bind(project).bind(cohort).bind(reason).execute(&mut **tx).await?;
    if suspend {
        sqlx::query("UPDATE project_ai_screening_authority SET suspended_reason=$2,updated_at=now() WHERE project_id=$1")
        .bind(project).bind(reason).execute(&mut **tx).await?;
    }
    let actor = Actor::new(ActorKind::System, "ai-first-safety")
        .map_err(|e| AiFirstError::Invalid(e.to_string()))?;
    ledger(
        tx,
        project,
        cohort,
        "ai_first_authority_reduced",
        json!({"reason":reason,"suspended":suspend}),
        &actor,
    )
    .await?;
    Ok(())
}

pub async fn recover_ai_first_cohort(
    pool: &PgPool,
    project: Uuid,
    cohort: Uuid,
    actor: &Actor,
) -> Result<(), AiFirstError> {
    user(actor)?;
    let mut tx = pool.begin().await?;
    lock_project(&mut tx, project).await?;
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM ai_screening_cohorts WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(project)
    .bind(cohort)
    .fetch_optional(&mut *tx)
    .await?;
    status.ok_or(AiFirstError::NotFound)?;
    invalidate_in_transaction(&mut tx, project, cohort, "owner_recovery", false).await?;
    // Recovering the current cohort also withdraws its owner ceiling. Recovering
    // historical work must not disable a separately approved active cohort.
    sqlx::query(
        "UPDATE project_ai_screening_authority SET ceiling='off',updated_at=now()
       WHERE project_id=$1 AND NOT EXISTS(SELECT 1 FROM ai_screening_cohorts
       WHERE project_id=$1 AND status IN ('open','closed','auditing','passed'))",
    )
    .bind(project)
    .execute(&mut *tx)
    .await?;
    ledger(
        &mut tx,
        project,
        cohort,
        "ai_first_cohort_recovered",
        json!({}),
        actor,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Reopen only automation events that remain canonical; a later human decision
/// always wins. Use the normal undo path, retaining both events and revisions.
async fn reopen_finalized(
    tx: &mut Transaction<'_, Postgres>,
    project: Uuid,
    cohort: Uuid,
    reason: &str,
) -> Result<(), AiFirstError> {
    let protocol:Option<Uuid>=sqlx::query_scalar("SELECT id FROM protocol_versions WHERE project_id=$1 AND status='published' ORDER BY version DESC LIMIT 1")
        .bind(project).fetch_optional(&mut **tx).await?;
    let rows=sqlx::query("SELECT d.report_id,s.revision FROM ai_screening_dispositions d JOIN screening_state s
       ON s.project_id=d.project_id AND s.report_id=d.report_id WHERE d.project_id=$1 AND d.cohort_id=$2
       AND d.finalized_event_id=s.last_event_id AND s.title_abstract_status='exclude' ORDER BY d.report_id")
        .bind(project).bind(cohort).fetch_all(&mut **tx).await?;
    let actor = Actor::new(ActorKind::System, "ai-first-safety")
        .map_err(|e| AiFirstError::Invalid(e.to_string()))?;
    for row in rows {
        let protocol = protocol.ok_or_else(|| {
            AiFirstError::Refused("cannot reopen without a published protocol".into())
        })?;
        crate::screening::undo_screening_in_transaction(
            tx,
            deepref_application::UndoScreeningCommand {
                project_id: project.into(),
                report_id: row.get::<Uuid, _>("report_id").into(),
                stage: deepref_domain::ScreeningStage::TitleAbstract,
                protocol_version_id: protocol.into(),
                expected_revision: row.get("revision"),
                notes: Some(format!("AI-first cohort {cohort} reopened: {reason}")),
                actor: actor.clone(),
            },
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn protocol_amended(
    tx: &mut Transaction<'_, Postgres>,
    project: Uuid,
) -> Result<(), AiFirstError> {
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM ai_screening_cohorts WHERE project_id=$1 AND status IN ('open','closed','auditing','passed','finalized') ORDER BY id")
        .bind(project).fetch_all(&mut **tx).await?;
    for id in ids {
        invalidate_in_transaction(tx, project, id, "protocol_changed", true).await?;
    }
    Ok(())
}

/// Changes to a completed reference or a discovered automation miss demote
/// immediately. A later human event remains canonical while other automated
/// exclusions reopen through the normal undo path.
pub(crate) async fn human_state_changed(
    tx: &mut Transaction<'_, Postgres>,
    project: Uuid,
    report: Uuid,
    actor: &Actor,
    was_undo: bool,
) -> Result<(), AiFirstError> {
    if actor.kind() != ActorKind::User {
        return Ok(());
    }
    let rows=sqlx::query("SELECT c.id,c.status,c.sampling_seed,m.sampled,s.title_abstract_status,
      EXISTS(SELECT 1 FROM ai_screening_dispositions d WHERE d.cohort_id=c.id AND d.report_id=m.report_id
        AND d.finalized_event_id IS NOT NULL) AS automated
      FROM ai_screening_cohort_members m JOIN ai_screening_cohorts c ON c.id=m.cohort_id
      LEFT JOIN screening_state s ON s.project_id=m.project_id AND s.report_id=m.report_id
      WHERE m.project_id=$1 AND m.report_id=$2 AND c.status IN ('open','closed','auditing','passed','finalized')")
        .bind(project).bind(report).fetch_all(&mut **tx).await?;
    for row in rows {
        let status = row.get::<String, _>("status");
        let cohort = row.get::<Uuid, _>("id");
        let drift = status == "finalized"
            && row.get::<bool, _>("automated")
            && row
                .get::<Option<String>, _>("title_abstract_status")
                .as_deref()
                != Some("exclude");
        let changed_reference = row.get::<Option<Uuid>, _>("sampling_seed").is_some()
            && !row.get::<bool, _>("automated")
            && (status == "finalized"
                || was_undo
                || !row.get::<bool, _>("sampled")
                || status == "passed");
        if drift || changed_reference {
            invalidate_in_transaction(
                tx,
                project,
                cohort,
                if drift {
                    "automation_miss_discovered"
                } else {
                    "frozen_reference_changed"
                },
                true,
            )
            .await?;
        }
    }
    Ok(())
}
