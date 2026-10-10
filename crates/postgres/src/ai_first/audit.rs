use super::cohorts::user;
use super::{AiFirstError, invalidate_in_transaction, ledger, lock_project};
use deepref_application::{
    ScreenReportCommand,
    ai_first::{AuditDesign, AuditResult},
};
use deepref_domain::{
    Actor, ActorKind, ProjectId, ProtocolVersionId, ReportId, ScreeningDecision, ScreeningStage,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

fn design(row: &sqlx::postgres::PgRow) -> Result<AuditDesign, AiFirstError> {
    let count = |key| -> Result<u32, AiFirstError> {
        row.try_get::<i32, _>(key)?
            .try_into()
            .map_err(|_| AiFirstError::Refused("invalid stored audit count".into()))
    };
    AuditDesign {
        population: count("population")?,
        reference_relevant: count("reference_relevant")?,
        sample_size: count("sample_size")?,
        target_percent: count("target_percent")?,
        alpha_billionths: count("alpha_billionths")?,
    }
    .validate()
    .map_err(|e| AiFirstError::Refused(e.to_string()))
}

/// Unbiased Fisher-Yates using SHA-256 counter expansion and rejection sampling.
/// The unpredictable seed is generated once in the draw transaction and persisted.
fn sample_order(ids: &mut [Uuid], seed: Uuid) {
    shuffle(ids, seed, "ai-first-fisher-yates-v1");
}

fn shuffle(ids: &mut [Uuid], seed: Uuid, domain: &str) {
    let mut counter = 0u64;
    for i in (1..ids.len()).rev() {
        let range = (i + 1) as u64;
        let upper = u64::MAX - u64::MAX % range;
        let value = loop {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(format!("{domain}:{seed}:{counter}").as_bytes());
            counter += 1;
            let value = hash
                .iter()
                .take(8)
                .fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
            if value < upper {
                break value % range;
            }
        };
        ids.swap(i, value as usize);
    }
}

pub async fn draw_ai_first_audit(
    pool: &PgPool,
    project: Uuid,
    cohort: Uuid,
    requested_sample: Option<u32>,
    actor: &Actor,
) -> Result<u32, AiFirstError> {
    user(actor)?;
    let mut tx = pool.begin().await?;
    lock_project(&mut tx, project).await?;
    let c =
        sqlx::query("SELECT * FROM ai_screening_cohorts WHERE project_id=$1 AND id=$2 FOR UPDATE")
            .bind(project)
            .bind(cohort)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AiFirstError::NotFound)?;
    if c.get::<String, _>("status") != "closed"
        || c.get::<Option<Uuid>, _>("sampling_seed").is_some()
    {
        return Err(AiFirstError::Refused(
            "close the cohort; only one audit draw is permitted".into(),
        ));
    }
    if !super::routing::ensure_current(pool, &mut tx, &c).await? {
        tx.commit().await?;
        return Err(AiFirstError::Refused(
            "cohort identity or protocol changed".into(),
        ));
    }
    // Freeze the non-quarantine human reference, including pre-draw targeted rescues.
    let human=sqlx::query("SELECT m.report_id,s.title_abstract_status,s.revision,e.id AS event_id,e.actor_kind,r.title,r.abstract_text
       FROM ai_screening_cohort_members m
       JOIN reports r ON r.id=m.report_id
       LEFT JOIN screening_state s ON s.project_id=m.project_id AND s.report_id=m.report_id
       LEFT JOIN LATERAL (SELECT id,actor_kind,decision FROM screening_events e WHERE e.project_id=m.project_id
         AND e.report_id=m.report_id AND e.stage='title_abstract' ORDER BY e.created_at DESC,e.id DESC LIMIT 1) e ON true
       WHERE m.cohort_id=$1 AND NOT EXISTS(SELECT 1 FROM ai_screening_dispositions d
         WHERE d.cohort_id=m.cohort_id AND d.report_id=m.report_id AND d.voided_at IS NULL AND d.finalized_at IS NULL)
       ORDER BY m.report_id")
        .bind(cohort).fetch_all(&mut *tx).await?;
    let mut retained = 0u32;
    let mut snapshot = Vec::new();
    let mut controls = Vec::new();
    for row in human {
        let status = row
            .get::<Option<String>, _>("title_abstract_status")
            .unwrap_or_else(|| "unscreened".into());
        if status == "unscreened"
            || row.get::<Option<String>, _>("actor_kind").as_deref() != Some("user")
        {
            return Err(AiFirstError::Refused(
                "complete human screening outside quarantine before drawing".into(),
            ));
        }
        if matches!(status.as_str(), "include" | "maybe") {
            retained += 1;
        }
        controls.push(row.get::<Uuid, _>("report_id"));
        snapshot.push(
            json!({"report_id":row.get::<Uuid,_>("report_id"),"decision":status,
            "revision":row.get::<i64,_>("revision"),"event_id":row.get::<Uuid,_>("event_id"),
            "source_snapshot":{"title":row.get::<Option<String>,_>("title"),"abstract_text":row.get::<Option<String>,_>("abstract_text")}}),
        );
    }
    let mut ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT report_id FROM ai_screening_dispositions
       WHERE cohort_id=$1 AND voided_at IS NULL AND finalized_at IS NULL ORDER BY report_id",
    )
    .bind(cohort)
    .fetch_all(&mut *tx)
    .await?;
    let population = u32::try_from(ids.len())
        .map_err(|_| AiFirstError::Invalid("cohort exceeds population limit".into()))?;
    if population == 0 {
        return Err(AiFirstError::Refused(
            "no quarantine remains to audit".into(),
        ));
    }
    // Never reset this project-wide counter after amendment, failure or recovery.
    let ordinal: i32 = sqlx::query_scalar(
        "UPDATE project_ai_screening_authority SET next_audit_ordinal=next_audit_ordinal+1
      WHERE project_id=$1 AND next_audit_ordinal<=25 RETURNING next_audit_ordinal-1",
    )
    .bind(project)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        AiFirstError::Refused("review-wide audit allocation exhausted; use human screening".into())
    })?;
    let alpha = 50_000_000u32 >> ordinal;
    let mut d = AuditDesign {
        population,
        reference_relevant: retained,
        sample_size: 0,
        target_percent: c.get::<i32, _>("target_percent") as u32,
        alpha_billionths: alpha,
    };
    let minimum = d
        .zero_miss_sample()
        .map_err(|e| AiFirstError::Invalid(e.to_string()))?;
    d.sample_size = requested_sample.unwrap_or(minimum);
    d.validate()
        .map_err(|e| AiFirstError::Invalid(e.to_string()))?;
    if d.sample_size < minimum {
        return Err(AiFirstError::Invalid(format!(
            "sample must be at least {minimum}; this assumes zero observed misses"
        )));
    }
    if controls.is_empty() && d.sample_size > 0 {
        return Err(AiFirstError::Refused(
            "blind audit requires non-quarantine controls; recover to ordinary human screening"
                .into(),
        ));
    }
    sqlx::query("UPDATE ai_screening_cohort_members m SET quarantine_frame=EXISTS(SELECT 1 FROM ai_screening_dispositions d WHERE d.cohort_id=m.cohort_id AND d.report_id=m.report_id AND d.voided_at IS NULL AND d.finalized_at IS NULL) WHERE m.cohort_id=$1")
        .bind(cohort).execute(&mut *tx).await?;
    let seed = Uuid::new_v4();
    sample_order(&mut ids, seed);
    for id in ids.iter().take(d.sample_size as usize) {
        sqlx::query("UPDATE ai_screening_cohort_members SET sampled=true WHERE cohort_id=$1 AND report_id=$2")
            .bind(cohort).bind(id).execute(&mut *tx).await?;
    }
    shuffle(&mut controls, seed, "ai-first-controls-v1");
    for id in controls.iter().take(d.sample_size as usize) {
        sqlx::query("UPDATE ai_screening_cohort_members SET audit_control=true WHERE cohort_id=$1 AND report_id=$2")
            .bind(cohort).bind(id).execute(&mut *tx).await?;
    }
    let mut blind_tasks: Vec<_> = ids
        .iter()
        .take(d.sample_size as usize)
        .chain(controls.iter().take(d.sample_size as usize))
        .copied()
        .collect();
    blind_tasks.sort();
    shuffle(&mut blind_tasks, seed, "ai-first-blind-queue-v1");
    for (order, id) in blind_tasks.iter().enumerate() {
        sqlx::query("UPDATE ai_screening_cohort_members SET audit_order=$3 WHERE cohort_id=$1 AND report_id=$2")
            .bind(cohort).bind(id).bind(order as i32).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE ai_screening_cohorts SET status='auditing',audit_ordinal=$2,alpha_billionths=$3,
      reference_relevant=$4,population=$5,sample_size=$6,sampling_seed=$7,
      sampling_algorithm='sha256-counter-rejection-fisher-yates-controls-v2',reference_snapshot=$8,
      frame_snapshot=(SELECT jsonb_agg(to_jsonb(m) ORDER BY m.report_id) FROM ai_screening_cohort_members m WHERE m.cohort_id=$1) WHERE id=$1")
        .bind(cohort).bind(ordinal).bind(alpha as i32).bind(retained as i32).bind(population as i32)
        .bind(d.sample_size as i32).bind(seed).bind(json!(snapshot)).execute(&mut *tx).await?;
    // Keep sample membership and seed private until terminal status; counts only.
    ledger(&mut tx,project,cohort,"ai_first_audit_drawn",json!({"design":d,"planning":"zero_miss_minimum",
       "reference_rule":"two_distinct_blinded_actors_or_include_maybe","review_wide_alpha_spending":true}),actor).await?;
    tx.commit().await?;
    Ok(d.sample_size)
}

pub(crate) enum AuditLabelOutcome {
    NotAudit,
    Pending,
}

/// Record labels before scientific state. The first reviewer cannot reveal either
/// opinion to the second. Canonical sample decisions wait until evaluation;
/// Ordinary controls preserve the reference; new positives invalidate it before inference.
pub(crate) async fn record_audit_label(
    tx: &mut Transaction<'_, Postgres>,
    command: &ScreenReportCommand,
) -> Result<AuditLabelOutcome, AiFirstError> {
    if command.stage != ScreeningStage::TitleAbstract {
        return Ok(AuditLabelOutcome::NotAudit);
    }
    let project = command.project_id.as_uuid();
    let report = command.report_id.as_uuid();
    let row = sqlx::query(
        "SELECT c.id,c.status,m.sampled,m.audit_control,c.sampling_seed FROM ai_screening_cohort_members m
       JOIN ai_screening_cohorts c ON c.id=m.cohort_id WHERE m.project_id=$1 AND m.report_id=$2
       AND c.status IN ('open','closed','auditing','passed') FOR UPDATE OF c",
    )
    .bind(project)
    .bind(report)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(AuditLabelOutcome::NotAudit);
    };
    let cohort = row.get::<Uuid, _>("id");
    if row.get::<String, _>("status") == "auditing"
        && !(row.get::<bool, _>("sampled") || row.get::<bool, _>("audit_control"))
    {
        return Err(AiFirstError::Refused(
            "complete or recover the blind audit before editing cohort records".into(),
        ));
    }
    if row.get::<String, _>("status") != "auditing"
        || !(row.get::<bool, _>("sampled") || row.get::<bool, _>("audit_control"))
    {
        if row.get::<Option<Uuid>, _>("sampling_seed").is_some() {
            invalidate_in_transaction(tx, project, cohort, "frozen_reference_changed", true)
                .await?;
        } else {
            sqlx::query("UPDATE ai_screening_dispositions SET voided_at=clock_timestamp(),void_reason='human_rescue'
              WHERE cohort_id=$1 AND report_id=$2 AND voided_at IS NULL AND finalized_at IS NULL")
                .bind(cohort).bind(report).execute(&mut **tx).await?;
        }
        return Ok(AuditLabelOutcome::NotAudit);
    }
    user(&command.actor)?;
    if command.expected_revision != 0 {
        return Err(AiFirstError::Refused(
            "blind labels require the displayed revision".into(),
        ));
    }
    let exposed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ai_opinion_exposures WHERE project_id=$1
      AND report_id=$2 AND stage='title_abstract' AND exposure_possible_at<=clock_timestamp()
      AND (audience_actor_id IS NULL OR (audience_actor_kind='user' AND audience_actor_id=$3)))",
    )
    .bind(project)
    .bind(report)
    .bind(command.actor.id())
    .fetch_one(&mut **tx)
    .await?;
    if exposed {
        invalidate_in_transaction(tx, project, cohort, "audit_label_exposed", true).await?;
        return Ok(AuditLabelOutcome::Pending);
    }
    let fresh: bool = sqlx::query_scalar("SELECT ai_first_fresh_auditor($1,$2)")
        .bind(cohort)
        .bind(command.actor.id())
        .fetch_one(&mut **tx)
        .await?;
    if !fresh {
        return Err(AiFirstError::Refused(
            "audit requires a fresh reviewer who has not seen earlier cohort decisions".into(),
        ));
    }
    let labels: Vec<String> = sqlx::query_scalar(
        "SELECT decision FROM ai_screening_audit_labels WHERE cohort_id=$1 AND report_id=$2",
    )
    .bind(cohort)
    .bind(report)
    .fetch_all(&mut **tx)
    .await?;
    if labels.len() >= 2 {
        return Err(AiFirstError::Refused(
            "reference labels are already complete".into(),
        ));
    }
    let decision = match command.decision {
        ScreeningDecision::Include => "include",
        ScreeningDecision::Maybe => "maybe",
        ScreeningDecision::Exclude => "exclude",
    };
    let changed = sqlx::query(
        "INSERT INTO ai_screening_audit_labels(project_id,cohort_id,report_id,actor_id,decision)
      VALUES($1,$2,$3,$4,$5) ON CONFLICT(cohort_id,report_id,actor_id) DO NOTHING",
    )
    .bind(project)
    .bind(cohort)
    .bind(report)
    .bind(command.actor.id())
    .bind(decision)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(AiFirstError::Refused("this reviewer already supplied a reference label; another reviewer must decide independently".into()));
    }
    Ok(AuditLabelOutcome::Pending)
}

async fn write_reference_decisions(
    tx: &mut Transaction<'_, Postgres>,
    c: &sqlx::postgres::PgRow,
    rescue_controls: bool,
) -> Result<(), AiFirstError> {
    let cohort = c.get::<Uuid, _>("id");
    let project = c.get::<Uuid, _>("project_id");
    let references = sqlx::query("SELECT m.report_id,COALESCE(s.revision,0) AS evaluated_revision,bool_or(l.decision<>'exclude') AS retained,
      min(l.actor_id) AS actor_id FROM ai_screening_cohort_members m LEFT JOIN screening_state s ON s.project_id=m.project_id AND s.report_id=m.report_id JOIN ai_screening_audit_labels l
      ON l.cohort_id=m.cohort_id AND l.report_id=m.report_id WHERE m.cohort_id=$1 AND (m.sampled OR ($2 AND m.audit_control AND s.title_abstract_status='exclude'))
      GROUP BY m.report_id,s.revision,m.sampled HAVING m.sampled OR bool_or(l.decision<>'exclude') ORDER BY m.report_id")
        .bind(cohort).bind(rescue_controls).fetch_all(&mut **tx).await?;
    for row in references {
        sqlx::query("UPDATE ai_screening_dispositions SET voided_at=clock_timestamp(),void_reason='audit_human_reference'
          WHERE cohort_id=$1 AND report_id=$2 AND voided_at IS NULL AND finalized_at IS NULL")
            .bind(cohort).bind(row.get::<Uuid,_>("report_id")).execute(&mut **tx).await?;
        crate::screening::write_audit_reference(tx, ScreenReportCommand {
            project_id:project.into(), report_id:row.get::<Uuid,_>("report_id").into(),
            stage:ScreeningStage::TitleAbstract,
            decision:if row.get::<bool,_>("retained") {ScreeningDecision::Include} else {ScreeningDecision::Exclude},
            exclusion_reason_id:None, protocol_version_id:c.get::<Uuid,_>("protocol_version_id").into(),
            expected_revision:row.get("evaluated_revision"),
            notes:Some(json!({"ai_first_cohort":cohort,"reference_rule":"dual_or","raw_labels":"ai_screening_audit_labels"}).to_string()),
            actor:Actor::new(ActorKind::User,row.get::<String,_>("actor_id")).map_err(|e|AiFirstError::Invalid(e.to_string()))?,
        }).await?;
    }
    Ok(())
}

pub async fn evaluate_ai_first_audit(
    pool: &PgPool,
    project: Uuid,
    cohort: Uuid,
    actor: &Actor,
) -> Result<AuditResult, AiFirstError> {
    user(actor)?;
    let mut tx = pool.begin().await?;
    lock_project(&mut tx, project).await?;
    let c =
        sqlx::query("SELECT * FROM ai_screening_cohorts WHERE project_id=$1 AND id=$2 FOR UPDATE")
            .bind(project)
            .bind(cohort)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AiFirstError::NotFound)?;
    if c.get::<String, _>("status") != "auditing" {
        return Err(AiFirstError::Refused(
            "the audit has already been evaluated or invalidated".into(),
        ));
    }
    if !super::routing::ensure_current(pool, &mut tx, &c).await? {
        tx.commit().await?;
        return Err(AiFirstError::Refused(
            "cohort identity or protocol changed".into(),
        ));
    }
    let incomplete:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ai_screening_cohort_members m WHERE m.cohort_id=$1 AND (m.sampled OR m.audit_control)
       AND (SELECT count(*) FROM ai_screening_audit_labels l WHERE l.cohort_id=m.cohort_id AND l.report_id=m.report_id)<>2)")
        .bind(cohort).fetch_one(&mut *tx).await?;
    if incomplete {
        return Err(AiFirstError::Refused(
            "two independent labels are required for every interleaved audit record".into(),
        ));
    }
    let new_control_positive: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ai_screening_cohort_members m
      JOIN screening_state s ON s.project_id=m.project_id AND s.report_id=m.report_id
      JOIN ai_screening_audit_labels l ON l.cohort_id=m.cohort_id AND l.report_id=m.report_id
      WHERE m.cohort_id=$1 AND m.audit_control AND s.title_abstract_status='exclude' AND l.decision<>'exclude')")
        .bind(cohort).fetch_one(&mut *tx).await?;
    if new_control_positive {
        invalidate_in_transaction(
            &mut tx,
            project,
            cohort,
            "control_reference_positive_found",
            true,
        )
        .await?;
        write_reference_decisions(&mut tx, &c, true).await?;
        ledger(
            &mut tx,
            project,
            cohort,
            "ai_first_control_rescue",
            json!({"statistical_look":false,"reference_changed":true}),
            actor,
        )
        .await?;
        tx.commit().await?;
        return Err(AiFirstError::Refused("new positive control changes the frozen human reference; cohort returned to human review without a statistical look".into()));
    }
    let k:i64=sqlx::query_scalar("SELECT count(DISTINCT l.report_id) FROM ai_screening_audit_labels l JOIN ai_screening_cohort_members m ON m.cohort_id=l.cohort_id AND m.report_id=l.report_id WHERE l.cohort_id=$1 AND m.sampled AND l.decision IN ('include','maybe')")
        .bind(cohort).fetch_one(&mut *tx).await?;
    let result = design(&c)?
        .evaluate(k as u32)
        .map_err(|e| AiFirstError::Invalid(e.to_string()))?;
    sqlx::query(
        "UPDATE ai_screening_cohorts SET status=$2,result=$3,evaluated_at=now() WHERE id=$1",
    )
    .bind(cohort)
    .bind(if result.passed { "passed" } else { "failed" })
    .bind(serde_json::to_value(&result)?)
    .execute(&mut *tx)
    .await?;
    write_reference_decisions(&mut tx, &c, false).await?;
    if !result.passed {
        sqlx::query("UPDATE ai_screening_dispositions SET voided_at=clock_timestamp(),void_reason='audit_failed'
          WHERE cohort_id=$1 AND voided_at IS NULL AND finalized_at IS NULL").bind(cohort).execute(&mut *tx).await?;
        sqlx::query("UPDATE project_ai_screening_authority SET suspended_reason='audit_failed',updated_at=now() WHERE project_id=$1")
            .bind(project).execute(&mut *tx).await?;
    }
    ledger(
        &mut tx,
        project,
        cohort,
        "ai_first_audit_evaluated",
        json!({"design":design(&c)?,"result":result,
       "claim":"conditional_reference_retention_only"}),
        actor,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn finalize_ai_first_cohort(
    pool: &PgPool,
    project: Uuid,
    cohort: Uuid,
    acknowledge_reference_limits: bool,
    actor: &Actor,
) -> Result<u32, AiFirstError> {
    user(actor)?;
    if !acknowledge_reference_limits {
        return Err(AiFirstError::Invalid(
            "acknowledge that reference retention does not guarantee true recall".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    lock_project(&mut tx, project).await?;
    let c =
        sqlx::query("SELECT * FROM ai_screening_cohorts WHERE project_id=$1 AND id=$2 FOR UPDATE")
            .bind(project)
            .bind(cohort)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AiFirstError::NotFound)?;
    if c.get::<String, _>("status") != "passed" {
        return Err(AiFirstError::Refused(
            "only a passing, current cohort can be finalized".into(),
        ));
    }
    if !super::routing::ensure_current(pool, &mut tx, &c).await? {
        tx.commit().await?;
        return Err(AiFirstError::Refused(
            "cohort identity or protocol changed".into(),
        ));
    }
    let ceiling: bool = sqlx::query_scalar(
        "SELECT ceiling='cohort_finalization' AND suspended_reason IS NULL
       FROM project_ai_screening_authority WHERE project_id=$1",
    )
    .bind(project)
    .fetch_one(&mut *tx)
    .await?;
    if !ceiling {
        return Err(AiFirstError::Refused(
            "owner ceiling does not permit finalization".into(),
        ));
    }
    let result: AuditResult = serde_json::from_value(c.get::<Value, _>("result"))?;
    let observed: i64 = sqlx::query_scalar("SELECT count(DISTINCT l.report_id) FROM ai_screening_audit_labels l JOIN ai_screening_cohort_members m ON m.cohort_id=l.cohort_id AND m.report_id=l.report_id WHERE l.cohort_id=$1 AND m.sampled AND l.decision IN ('include','maybe')")
        .bind(cohort).fetch_one(&mut *tx).await?;
    if u32::try_from(observed).ok() != Some(result.observed_relevant) {
        return Err(AiFirstError::Refused(
            "stored inference differs from reference labels".into(),
        ));
    }
    let recomputed = design(&c)?
        .evaluate(result.observed_relevant)
        .map_err(|e| AiFirstError::Refused(e.to_string()))?;
    if !recomputed.passed || result != recomputed {
        return Err(AiFirstError::Refused(
            "stored inference does not reproduce".into(),
        ));
    }
    let approval = Uuid::new_v4();
    let rows=sqlx::query("SELECT d.id,d.report_id,d.evaluated_revision FROM ai_screening_dispositions d
       JOIN ai_screening_cohort_members m ON m.cohort_id=d.cohort_id AND m.report_id=d.report_id
       WHERE d.cohort_id=$1 AND NOT m.sampled AND d.voided_at IS NULL AND d.finalized_at IS NULL ORDER BY d.report_id FOR UPDATE OF d")
        .bind(cohort).fetch_all(&mut *tx).await?;
    let automation = Actor::new(ActorKind::Automation, format!("ai-first-cohort:{cohort}"))
        .map_err(|e| AiFirstError::Invalid(e.to_string()))?;
    // Terminal status before scientific writes prevents the human-reference hook
    // from interpreting these approved cohort writes as new reference judgments.
    sqlx::query("UPDATE ai_screening_cohorts SET status='finalized',final_approval_id=$2,finalized_by=$3,finalized_at=now() WHERE id=$1")
        .bind(cohort).bind(approval).bind(actor.id()).execute(&mut *tx).await?;
    for row in &rows {
        let snapshot=crate::screening::screen_report_in_transaction(&mut tx,ScreenReportCommand {
            project_id:ProjectId::new(project),report_id:ReportId::new(row.get("report_id")),
            stage:ScreeningStage::TitleAbstract,decision:ScreeningDecision::Exclude,exclusion_reason_id:None,
            protocol_version_id:ProtocolVersionId::new(c.get("protocol_version_id")),expected_revision:row.get("evaluated_revision"),
            notes:Some(json!({"ai_first_cohort":cohort,"approval_id":approval,"policy_version":1,
                "semantic_bundle_hash":c.get::<String,_>("semantic_bundle_hash"),"approved_by":actor.id()}).to_string()),actor:automation.clone(),
        }).await?;
        sqlx::query("UPDATE ai_screening_dispositions SET finalized_at=now(),finalized_event_id=$2 WHERE id=$1")
            .bind(row.get::<Uuid,_>("id")).bind(snapshot.last_event_id).execute(&mut *tx).await?;
    }
    ledger(
        &mut tx,
        project,
        cohort,
        "ai_first_finalization_approved",
        json!({"approval_id":approval,"design":design(&c)?,
        "result":result,"automated_exclusions":rows.len(),"reference_limits_acknowledged":true,
        "actor_identity":"self_asserted"}),
        actor,
    )
    .await?;
    tx.commit().await?;
    Ok(rows.len() as u32)
}

/// Detect deleted membership, changed denominators or report edits without a
/// screening revision. The snapshot is the authoritative frame, never survivors.
pub(crate) async fn frozen_evidence_current(
    tx: &mut Transaction<'_, Postgres>,
    c: &sqlx::postgres::PgRow,
) -> Result<bool, AiFirstError> {
    let cohort = c.get::<Uuid, _>("id");
    let contaminated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ai_screening_audit_labels l JOIN ai_opinion_exposures e
       ON e.project_id=l.project_id AND e.report_id=l.report_id
       WHERE l.cohort_id=$1 AND e.exposure_possible_at<=l.created_at
       AND (e.audience_actor_id IS NULL OR (e.audience_actor_kind='user' AND e.audience_actor_id=l.actor_id)))")
        .bind(cohort).fetch_one(&mut **tx).await?;
    if contaminated {
        return Ok(false);
    }
    let sources_valid:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM ai_screening_dispositions d
       JOIN reports r ON r.id=d.report_id WHERE d.cohort_id=$1
       AND d.source_snapshot IS DISTINCT FROM jsonb_build_object('title',r.title,'abstract_text',r.abstract_text))")
        .bind(cohort).fetch_one(&mut **tx).await?;
    if !sources_valid {
        return Ok(false);
    }
    if c.get::<Option<Uuid>, _>("sampling_seed").is_none() {
        return Ok(true);
    }
    let frame:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(to_jsonb(m) ORDER BY m.report_id),'[]'::jsonb) FROM ai_screening_cohort_members m WHERE m.cohort_id=$1")
        .bind(cohort).fetch_one(&mut **tx).await?;
    if Some(frame) != c.get::<Option<Value>, _>("frame_snapshot") {
        return Ok(false);
    }
    #[derive(serde::Deserialize)]
    struct Reference {
        report_id: Uuid,
        decision: String,
        event_id: Uuid,
        source_snapshot: Value,
    }
    let references: Vec<Reference> = serde_json::from_value(c.get("reference_snapshot"))?;
    for reference in references {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM screening_state s JOIN reports r ON r.id=s.report_id WHERE s.project_id=$1 AND s.report_id=$2
          AND s.title_abstract_status=$3 AND jsonb_build_object('title',r.title,'abstract_text',r.abstract_text)=$4
          AND $5=(SELECT e.id FROM screening_events e WHERE e.project_id=s.project_id AND e.report_id=s.report_id
          AND e.stage='title_abstract' ORDER BY e.created_at DESC,e.id DESC LIMIT 1)
          AND EXISTS(SELECT 1 FROM screening_events e WHERE e.id=$5 AND e.actor_kind='user'))")
            .bind(c.get::<Uuid,_>("project_id")).bind(reference.report_id).bind(reference.decision).bind(reference.source_snapshot)
            .bind(reference.event_id).fetch_one(&mut **tx).await?;
        if !valid {
            return Ok(false);
        }
    }
    let x:i64=sqlx::query_scalar("SELECT count(*) FROM ai_screening_cohort_members m JOIN ai_screening_dispositions d ON d.cohort_id=m.cohort_id AND d.report_id=m.report_id WHERE m.cohort_id=$1 AND m.quarantine_frame")
        .bind(cohort).fetch_one(&mut **tx).await?;
    if x != i64::from(c.get::<i32, _>("population")) {
        return Ok(false);
    }
    let samples: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_screening_cohort_members WHERE cohort_id=$1 AND sampled",
    )
    .bind(cohort)
    .fetch_one(&mut **tx)
    .await?;
    let controls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_screening_cohort_members WHERE cohort_id=$1 AND audit_control",
    )
    .bind(cohort)
    .fetch_one(&mut **tx)
    .await?;
    let human_count = c
        .get::<Value, _>("reference_snapshot")
        .as_array()
        .map_or(0, Vec::len) as i64;
    Ok(
        samples == i64::from(c.get::<i32, _>("sample_size"))
            && controls == samples.min(human_count),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sampling_is_stable_unique_and_order_independent_after_sort() {
        let mut ids: Vec<_> = (1..=100).map(Uuid::from_u128).collect();
        let original = ids.clone();
        sample_order(&mut ids, Uuid::from_u128(123));
        let mut again = original.clone();
        sample_order(&mut again, Uuid::from_u128(123));
        assert_eq!(ids, again);
        assert_eq!(
            ids.iter()
                .take(10)
                .map(|id| id.as_u128())
                .collect::<Vec<_>>(),
            vec![65, 33, 60, 23, 10, 67, 2, 20, 82, 75],
            "matches the independent CSV reproduction tool"
        );
        assert_ne!(ids, original);
        ids.sort();
        assert_eq!(ids, original);
    }
}
