use std::collections::BTreeSet;

use deepref_application::RawAuthor;
use deepref_application::{
    DedupeCandidate, DedupeScore, FUZZY_PROPOSAL_THRESHOLD, FUZZY_SHORTLIST_LIMIT,
    ProposalDecision, ProposalKind, RecordResolutionAction, ResolveRecordCommand, score_candidate,
};
use deepref_domain::normalize_bibliography_title;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum DedupeError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("project not found")]
    ProjectNotFound,
    #[error("record not found in this project")]
    RecordNotFound,
    #[error("report is not part of this project")]
    ReportNotInProject,
    #[error("deduplication proposal not found in this project")]
    ProposalNotFound,
    #[error("proposal is no longer pending")]
    ProposalNotPending,
    #[error("create-new is not valid for identifier conflict proposals")]
    ConflictCreateNew,
    #[error("revert history has no effective resolution to undo")]
    RevertConflict,
    #[error("record identifiers point to different reports")]
    IdentifierConflict,
    #[error("resolution command is invalid: {0}")]
    InvalidCommand(String),
    #[error("failed to serialize deduplication metadata")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupeRunRequest {
    pub project_id: Uuid,
    pub limit: i64,
    pub actor_kind: String,
    pub actor_id: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DedupeRunSummary {
    pub processed: i64,
    pub auto_linked: i64,
    pub created_reports: i64,
    pub proposals_created: i64,
    pub conflicts: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DedupeProposal {
    pub id: Uuid,
    pub project_id: Uuid,
    pub record_id: Uuid,
    pub candidate_report_id: Option<Uuid>,
    pub proposal_kind: String,
    pub source_title: Option<String>,
    pub source_abstract: Option<String>,
    pub source_year: Option<i32>,
    pub source_authors: Value,
    pub source_identifiers: Value,
    pub candidate_title: Option<String>,
    pub candidate_year: Option<i32>,
    pub candidate_authors: Value,
    pub candidate_identifiers: Value,
    pub title_similarity: f64,
    pub year_match: Option<bool>,
    pub first_author_similarity: Option<f64>,
    pub exact_identifier_match: bool,
    pub conflicting_identifier: bool,
    pub score: f64,
    pub metadata: Value,
    pub status: String,
    pub revision: i64,
    pub reviewer_kind: Option<String>,
    pub reviewer_id: Option<String>,
    pub decided_at: Option<chrono::DateTime<chrono::Utc>>,
    pub decision_reason: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupeProposalCursor {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalDecisionRequest {
    pub project_id: Uuid,
    pub proposal_id: Uuid,
    pub decision: ProposalDecision,
    pub reason: String,
    pub actor_kind: String,
    pub actor_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionResult {
    pub record_id: Uuid,
    pub prior_report_id: Option<Uuid>,
    pub resolved_report_id: Option<Uuid>,
    pub action: String,
}

struct ResolutionLink<'a> {
    project_id: Uuid,
    record_id: Uuid,
    prior_report_id: Option<Uuid>,
    report_id: Uuid,
    action: &'a str,
    reason: &'a str,
    actor_kind: &'a str,
    actor_id: &'a str,
    proposal_id: Option<Uuid>,
}

struct ResolutionEvent<'a> {
    project_id: Uuid,
    record_id: Uuid,
    prior_report_id: Option<Uuid>,
    resolved_report_id: Option<Uuid>,
    action: &'a str,
    reason: &'a str,
    actor_kind: &'a str,
    actor_id: &'a str,
    proposal_id: Option<Uuid>,
    reverted_event_id: Option<Uuid>,
}

pub async fn run_deduplication(
    pool: &PgPool,
    request: DedupeRunRequest,
) -> Result<DedupeRunSummary, DedupeError> {
    let mut tx = pool.begin().await?;
    ensure_project(&mut tx, request.project_id).await?;

    // A project lock makes a bounded run and manual resolution serialize for
    // one review, while identifier locks below also protect cross-project
    // races around globally unique durable identifiers.
    lock_project(&mut tx, request.project_id).await?;
    refresh_project_report_titles(&mut tx, request.project_id).await?;
    let record_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM records
         WHERE project_id=$1 AND report_id IS NULL
         ORDER BY created_at,id
         LIMIT $2
         FOR UPDATE SKIP LOCKED",
    )
    .bind(request.project_id)
    .bind(request.limit)
    .fetch_all(&mut *tx)
    .await?;

    let mut summary = DedupeRunSummary::default();
    for record_id in record_ids {
        summary.processed += 1;
        let result = resolve_one_record(
            &mut tx,
            request.project_id,
            record_id,
            &request.actor_kind,
            &request.actor_id,
        )
        .await?;
        summary.auto_linked += result.auto_linked;
        summary.created_reports += result.created_report;
        summary.proposals_created += result.proposals;
        summary.conflicts += result.conflicts;
    }
    tx.commit().await?;
    Ok(summary)
}

#[derive(Debug, Clone, Copy, Default)]
struct OneRecordResult {
    auto_linked: i64,
    created_report: i64,
    proposals: i64,
    conflicts: i64,
}

struct SourceRecord {
    id: Uuid,
    title: Option<String>,
    abstract_text: Option<String>,
    publication_year: Option<i32>,
    journal: Option<String>,
    authors: Value,
    source_identifiers: Value,
    raw: Value,
}

async fn resolve_one_record(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    record_id: Uuid,
    actor_kind: &str,
    actor_id: &str,
) -> Result<OneRecordResult, DedupeError> {
    let row = sqlx::query(
        "SELECT id,title,abstract_text,publication_year,journal,authors,source_identifiers,raw
         FROM records WHERE project_id=$1 AND id=$2 AND report_id IS NULL FOR UPDATE",
    )
    .bind(project_id)
    .bind(record_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(OneRecordResult::default());
    };
    let source = SourceRecord {
        id: row.get("id"),
        title: row.get("title"),
        abstract_text: row.get("abstract_text"),
        publication_year: row.get("publication_year"),
        journal: row.get("journal"),
        authors: row.get("authors"),
        source_identifiers: row.get("source_identifiers"),
        raw: row.get("raw"),
    };

    let identifiers = lock_record_identifiers(tx, source.id).await?;

    let normalized_title = source
        .title
        .as_deref()
        .map(normalize_bibliography_title)
        .filter(|title| !title.is_empty());
    sqlx::query("UPDATE records SET normalized_title=$3 WHERE project_id=$1 AND id=$2")
        .bind(project_id)
        .bind(source.id)
        .bind(&normalized_title)
        .execute(&mut **tx)
        .await?;

    let matched_report_ids = matched_report_ids(tx, &identifiers).await?;
    if matched_report_ids.len() == 1 {
        attach_record_identifiers(tx, &source, matched_report_ids[0], false).await?;
        link_record(
            tx,
            ResolutionLink {
                project_id,
                record_id: source.id,
                prior_report_id: None,
                report_id: matched_report_ids[0],
                action: "auto_link",
                reason: "matched non-conflicting durable identifiers",
                actor_kind,
                actor_id,
                proposal_id: None,
            },
        )
        .await?;
        return Ok(OneRecordResult {
            auto_linked: 1,
            ..Default::default()
        });
    }
    if matched_report_ids.len() > 1 {
        let mut result = OneRecordResult::default();
        for candidate_report_id in &matched_report_ids {
            insert_proposal(
                tx,
                project_id,
                &source,
                Some(*candidate_report_id),
                ProposalKind::Conflict,
                &DedupeScore {
                    title_similarity: 0.0,
                    year_match: None,
                    first_author_similarity: None,
                    exact_identifier_match: true,
                    conflicting_identifier: true,
                    total: 0.0,
                },
                json!({
                    "reason": "durable identifiers point to different reports",
                    "matched_report_ids": matched_report_ids,
                }),
            )
            .await?;
            result.proposals += 1;
        }
        result.conflicts = 1;
        return Ok(result);
    }

    let Some(normalized_title) = normalized_title.as_deref() else {
        let report_id = create_report(tx, &source, None).await?;
        link_record(
            tx,
            ResolutionLink {
                project_id,
                record_id: source.id,
                prior_report_id: None,
                report_id,
                action: "create_report",
                reason: "no durable identifier or title candidate",
                actor_kind,
                actor_id,
                proposal_id: None,
            },
        )
        .await?;
        return Ok(OneRecordResult {
            created_report: 1,
            ..Default::default()
        });
    };

    let shortlist = shortlist_reports(tx, project_id, normalized_title).await?;
    let source_first_author = first_author_name(&source.authors);
    let source_year = source.publication_year;
    let mut scored = shortlist
        .into_iter()
        .map(|candidate| {
            let score = score_candidate(
                source.title.as_deref(),
                source_first_author.as_deref(),
                source_year,
                &candidate,
            );
            (candidate, score)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .1
            .total
            .partial_cmp(&left.1.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.report_id.as_uuid().cmp(&right.0.report_id.as_uuid()))
    });

    if let Some((candidate, score)) = scored
        .into_iter()
        .find(|(_, score)| score.total >= FUZZY_PROPOSAL_THRESHOLD && !score.exact_identifier_match)
    {
        insert_proposal(
            tx,
            project_id,
            &source,
            Some(candidate.report_id.as_uuid()),
            ProposalKind::Fuzzy,
            &score,
            json!({
                "threshold": FUZZY_PROPOSAL_THRESHOLD,
                "shortlist_limit": FUZZY_SHORTLIST_LIMIT,
                "method": "pg_trgm_shortlist_then_rapidfuzz_rerank",
            }),
        )
        .await?;
        return Ok(OneRecordResult {
            proposals: 1,
            ..Default::default()
        });
    }

    let report_id = create_report(tx, &source, Some(normalized_title.to_owned())).await?;
    link_record(
        tx,
        ResolutionLink {
            project_id,
            record_id: source.id,
            prior_report_id: None,
            report_id,
            action: "create_report",
            reason: "no credible candidate in bounded shortlist",
            actor_kind,
            actor_id,
            proposal_id: None,
        },
    )
    .await?;
    Ok(OneRecordResult {
        created_report: 1,
        ..Default::default()
    })
}

async fn ensure_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<(), DedupeError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id)
        .fetch_one(&mut **tx)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(DedupeError::ProjectNotFound)
    }
}

async fn lock_project(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("dedupe:project:{project_id}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn lock_identifier(
    tx: &mut Transaction<'_, Postgres>,
    scheme: &str,
    normalized_value: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("dedupe:identifier:{scheme}:{normalized_value}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn refresh_project_report_titles(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<(), sqlx::Error> {
    let rows = sqlx::query(
        "SELECT r.id,r.title
         FROM project_reports pr
         JOIN reports r ON r.id=pr.report_id
         WHERE pr.project_id=$1
         ORDER BY r.id
         FOR UPDATE OF r",
    )
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let title: Option<String> = row.get("title");
        let normalized_title = title
            .as_deref()
            .map(normalize_bibliography_title)
            .filter(|title| !title.is_empty());
        sqlx::query("UPDATE reports SET normalized_title=$2 WHERE id=$1")
            .bind(row.get::<Uuid, _>("id"))
            .bind(normalized_title)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordIdentifier {
    scheme: String,
    normalized_value: String,
}

async fn record_identifiers(
    tx: &mut Transaction<'_, Postgres>,
    record_id: Uuid,
) -> Result<Vec<RecordIdentifier>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT scheme,normalized_value FROM record_identifiers WHERE record_id=$1 ORDER BY scheme,normalized_value",
    )
    .bind(record_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| RecordIdentifier {
            scheme: row.get("scheme"),
            normalized_value: row.get("normalized_value"),
        })
        .collect())
}

async fn lock_record_identifiers(
    tx: &mut Transaction<'_, Postgres>,
    record_id: Uuid,
) -> Result<Vec<RecordIdentifier>, sqlx::Error> {
    let identifiers = record_identifiers(tx, record_id).await?;
    for identifier in &identifiers {
        lock_identifier(tx, &identifier.scheme, &identifier.normalized_value).await?;
    }
    Ok(identifiers)
}

async fn matched_report_ids(
    tx: &mut Transaction<'_, Postgres>,
    identifiers: &[RecordIdentifier],
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut matched = BTreeSet::new();
    for identifier in identifiers {
        let report_ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT report_id FROM report_identifiers WHERE scheme=$1 AND normalized_value=$2",
        )
        .bind(&identifier.scheme)
        .bind(&identifier.normalized_value)
        .fetch_all(&mut **tx)
        .await?;
        matched.extend(report_ids);
    }
    Ok(matched.into_iter().collect())
}

async fn attach_record_identifiers(
    tx: &mut Transaction<'_, Postgres>,
    source: &SourceRecord,
    report_id: Uuid,
    allow_conflicts: bool,
) -> Result<(), DedupeError> {
    let identifiers = record_identifiers(tx, source.id).await?;
    let source_values =
        serde_json::from_value::<Vec<Value>>(source.source_identifiers.clone()).unwrap_or_default();
    for identifier in identifiers {
        let owner = sqlx::query_scalar::<_, Uuid>(
            "SELECT report_id FROM report_identifiers
             WHERE scheme=$1 AND normalized_value=$2",
        )
        .bind(&identifier.scheme)
        .bind(&identifier.normalized_value)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(owner) = owner {
            if owner != report_id {
                if allow_conflicts {
                    continue;
                }
                return Err(DedupeError::IdentifierConflict);
            }
            continue;
        }
        let original = original_identifier_value(&source_values, &identifier);
        sqlx::query(
            "INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value)
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(Uuid::new_v4())
        .bind(report_id)
        .bind(&identifier.scheme)
        .bind(original)
        .bind(&identifier.normalized_value)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn shortlist_reports(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    normalized_title: &str,
) -> Result<Vec<DedupeCandidate>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT r.id,r.title,r.publication_year,r.authors
         FROM project_reports pr
         JOIN reports r ON r.id=pr.report_id
         WHERE pr.project_id=$1 AND r.normalized_title IS NOT NULL
           AND r.normalized_title % $2
         ORDER BY similarity(r.normalized_title,$2) DESC,r.id
         LIMIT $3",
    )
    .bind(project_id)
    .bind(normalized_title)
    .bind(FUZZY_SHORTLIST_LIMIT)
    .fetch_all(&mut **tx)
    .await?;
    let candidates = rows
        .into_iter()
        .map(|row| {
            let report_id: Uuid = row.get("id");
            let title: Option<String> = row.get("title");
            DedupeCandidate {
                report_id: report_id.into(),
                title,
                first_author: first_author_name(&row.get::<Value, _>("authors")),
                publication_year: row.get("publication_year"),
                exact_identifier_match: false,
                conflicting_identifier: false,
            }
        })
        .collect();
    Ok(candidates)
}

fn first_author_name(authors: &Value) -> Option<String> {
    let parsed = serde_json::from_value::<Vec<RawAuthor>>(authors.clone()).ok()?;
    let author = parsed.first()?;
    author
        .literal
        .clone()
        .or_else(|| author.family.clone())
        .or_else(|| author.given.clone())
}

async fn insert_proposal(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    source: &SourceRecord,
    candidate_report_id: Option<Uuid>,
    kind: ProposalKind,
    score: &DedupeScore,
    metadata: Value,
) -> Result<Uuid, DedupeError> {
    let proposal_id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO dedupe_proposals
         (id,project_id,record_id,candidate_report_id,proposal_kind,title_similarity,year_match,
          first_author_similarity,exact_identifier_match,conflicting_identifier,score,metadata)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
         ON CONFLICT (project_id,record_id,coalesce(candidate_report_id,'00000000-0000-0000-0000-000000000000'::uuid),proposal_kind)
         WHERE status='pending'
         DO UPDATE SET title_similarity=EXCLUDED.title_similarity,year_match=EXCLUDED.year_match,
           first_author_similarity=EXCLUDED.first_author_similarity,exact_identifier_match=EXCLUDED.exact_identifier_match,
           conflicting_identifier=EXCLUDED.conflicting_identifier,score=EXCLUDED.score,metadata=EXCLUDED.metadata,
           updated_at=now()
         RETURNING id",
    )
    .bind(proposal_id)
    .bind(project_id)
    .bind(source.id)
    .bind(candidate_report_id)
    .bind(kind.as_str())
    .bind(score.title_similarity)
    .bind(score.year_match)
    .bind(score.first_author_similarity)
    .bind(score.exact_identifier_match)
    .bind(score.conflicting_identifier)
    .bind(score.total)
    .bind(metadata)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.get("id"))
}

async fn create_report(
    tx: &mut Transaction<'_, Postgres>,
    source: &SourceRecord,
    normalized_title: Option<String>,
) -> Result<Uuid, DedupeError> {
    let report_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO reports
         (id,title,abstract_text,publication_year,journal,authors,normalized_title,raw)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(report_id)
    .bind(&source.title)
    .bind(&source.abstract_text)
    .bind(source.publication_year)
    .bind(&source.journal)
    .bind(&source.authors)
    .bind(normalized_title)
    .bind(&source.raw)
    .execute(&mut **tx)
    .await?;

    let identifiers = record_identifiers(tx, source.id).await?;
    let source_values =
        serde_json::from_value::<Vec<Value>>(source.source_identifiers.clone()).unwrap_or_default();
    for identifier in identifiers {
        let original = original_identifier_value(&source_values, &identifier);
        sqlx::query(
            "INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value)
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(Uuid::new_v4())
        .bind(report_id)
        .bind(&identifier.scheme)
        .bind(original)
        .bind(&identifier.normalized_value)
        .execute(&mut **tx)
        .await?;
    }
    Ok(report_id)
}

fn original_identifier_value(source_values: &[Value], identifier: &RecordIdentifier) -> String {
    source_values
        .iter()
        .find(|value| {
            value.get("scheme").and_then(Value::as_str) == Some(identifier.scheme.as_str())
                && value.get("normalized_value").and_then(Value::as_str)
                    == Some(identifier.normalized_value.as_str())
        })
        .and_then(|value| value.get("value"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| identifier.normalized_value.clone())
}

async fn link_record(
    tx: &mut Transaction<'_, Postgres>,
    link: ResolutionLink<'_>,
) -> Result<(), DedupeError> {
    sqlx::query("UPDATE records SET report_id=$3 WHERE project_id=$1 AND id=$2")
        .bind(link.project_id)
        .bind(link.record_id)
        .bind(link.report_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO project_reports (project_id,report_id,first_seen_record_id)
         VALUES ($1,$2,$3)
         ON CONFLICT (project_id,report_id) DO UPDATE SET
           first_seen_record_id=COALESCE(project_reports.first_seen_record_id,EXCLUDED.first_seen_record_id)",
    )
    .bind(link.project_id)
    .bind(link.report_id)
    .bind(link.record_id)
    .execute(&mut **tx)
    .await?;
    insert_resolution_event(
        tx,
        ResolutionEvent {
            project_id: link.project_id,
            record_id: link.record_id,
            prior_report_id: link.prior_report_id,
            resolved_report_id: Some(link.report_id),
            action: link.action,
            reason: link.reason,
            actor_kind: link.actor_kind,
            actor_id: link.actor_id,
            proposal_id: link.proposal_id,
            reverted_event_id: None,
        },
    )
    .await
}

pub async fn list_proposals(
    pool: &PgPool,
    project_id: Uuid,
    status: &str,
    cursor: Option<DedupeProposalCursor>,
    limit: i64,
) -> Result<Vec<DedupeProposal>, DedupeError> {
    let project_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
            .bind(project_id)
            .fetch_one(pool)
            .await?;
    if !project_exists {
        return Err(DedupeError::ProjectNotFound);
    }
    let rows = sqlx::query(
        "SELECT p.id,p.project_id,p.record_id,p.candidate_report_id,p.proposal_kind,
                p.title_similarity,p.year_match,p.first_author_similarity,p.exact_identifier_match,
                p.conflicting_identifier,p.score,p.metadata,p.status,p.revision,p.reviewer_kind,
                p.reviewer_id,p.decided_at,p.decision_reason,p.created_at,
                rec.title AS source_title,rec.abstract_text AS source_abstract,rec.publication_year AS source_year,
                rec.authors AS source_authors,rec.source_identifiers AS source_identifiers,
                candidate.title AS candidate_title,candidate.publication_year AS candidate_year,
                candidate.authors AS candidate_authors,
                coalesce((SELECT jsonb_agg(jsonb_build_object('scheme',ri.scheme,'value',ri.value,'normalized_value',ri.normalized_value)
                          ORDER BY ri.scheme,ri.normalized_value) FROM report_identifiers ri WHERE ri.report_id=candidate.id),'[]'::jsonb) AS candidate_identifiers
         FROM dedupe_proposals p
         JOIN records rec ON rec.project_id=p.project_id AND rec.id=p.record_id
         LEFT JOIN reports candidate ON candidate.id=p.candidate_report_id
         WHERE p.project_id=$1 AND p.status=$2
           AND ($3::timestamptz IS NULL OR (p.created_at,p.id)<($3,$4))
         ORDER BY p.created_at DESC,p.id DESC
         LIMIT $5",
    )
    .bind(project_id)
    .bind(status)
    .bind(cursor.as_ref().map(|value| value.created_at))
    .bind(cursor.as_ref().map(|value| value.id))
    .bind(limit + 1)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(proposal_from_row).collect())
}

pub async fn decide_proposal(
    pool: &PgPool,
    request: ProposalDecisionRequest,
) -> Result<ResolutionResult, DedupeError> {
    if request.reason.trim().is_empty() {
        return Err(DedupeError::InvalidCommand(
            "reason must not be empty".to_owned(),
        ));
    }
    validate_actor(&request.actor_kind, &request.actor_id)?;
    let mut tx = pool.begin().await?;
    ensure_project(&mut tx, request.project_id).await?;
    lock_project(&mut tx, request.project_id).await?;
    let proposal = sqlx::query(
        "SELECT id,record_id,candidate_report_id,proposal_kind,status,revision FROM dedupe_proposals
         WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(request.project_id)
    .bind(request.proposal_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DedupeError::ProposalNotFound)?;
    if proposal.get::<String, _>("status") != "pending" {
        return Err(DedupeError::ProposalNotPending);
    }
    let record_id: Uuid = proposal.get("record_id");
    let candidate_report_id: Option<Uuid> = proposal.get("candidate_report_id");
    let proposal_kind: String = proposal.get("proposal_kind");
    if matches!(request.decision, ProposalDecision::CreateNew) && proposal_kind == "conflict" {
        return Err(DedupeError::ConflictCreateNew);
    }
    let prior_report_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT report_id FROM records WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(request.project_id)
    .bind(record_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();

    let source = if matches!(request.decision, ProposalDecision::Reject) {
        None
    } else {
        let source = load_source_record(&mut tx, request.project_id, record_id).await?;
        lock_record_identifiers(&mut tx, record_id).await?;
        Some(source)
    };

    let resolved_report_id = match request.decision {
        ProposalDecision::Accept => {
            let report_id = candidate_report_id.ok_or_else(|| {
                DedupeError::InvalidCommand("accept requires a candidate report".to_owned())
            })?;
            ensure_report_membership(&mut tx, request.project_id, report_id).await?;
            attach_record_identifiers(
                &mut tx,
                source.as_ref().ok_or(DedupeError::RecordNotFound)?,
                report_id,
                proposal_kind == "conflict",
            )
            .await?;
            link_record(
                &mut tx,
                ResolutionLink {
                    project_id: request.project_id,
                    record_id,
                    prior_report_id,
                    report_id,
                    action: "accept_proposal",
                    reason: &request.reason,
                    actor_kind: &request.actor_kind,
                    actor_id: &request.actor_id,
                    proposal_id: Some(request.proposal_id),
                },
            )
            .await?;
            Some(report_id)
        }
        ProposalDecision::CreateNew => {
            let source = source.as_ref().ok_or(DedupeError::RecordNotFound)?;
            let identifiers = record_identifiers(&mut tx, record_id).await?;
            if !matched_report_ids(&mut tx, &identifiers).await?.is_empty() {
                return Err(DedupeError::IdentifierConflict);
            }
            let normalized_title = source.title.as_deref().map(normalize_bibliography_title);
            let report_id = create_report(&mut tx, source, normalized_title).await?;
            link_record(
                &mut tx,
                ResolutionLink {
                    project_id: request.project_id,
                    record_id,
                    prior_report_id,
                    report_id,
                    action: "create_new",
                    reason: &request.reason,
                    actor_kind: &request.actor_kind,
                    actor_id: &request.actor_id,
                    proposal_id: Some(request.proposal_id),
                },
            )
            .await?;
            Some(report_id)
        }
        ProposalDecision::Reject => None,
    };
    if !matches!(request.decision, ProposalDecision::Reject) {
        supersede_sibling_proposals(
            &mut tx,
            request.project_id,
            record_id,
            request.proposal_id,
            &request.reason,
            &request.actor_kind,
            &request.actor_id,
        )
        .await?;
    }
    let action = match request.decision {
        ProposalDecision::Reject => "reject_proposal",
        ProposalDecision::Accept => "accept_proposal",
        ProposalDecision::CreateNew => "create_new",
    };
    sqlx::query(
        "UPDATE dedupe_proposals
         SET status=$3,revision=revision+1,reviewer_kind=$4,reviewer_id=$5,
             decided_at=now(),decision_reason=$6,updated_at=now()
         WHERE project_id=$1 AND id=$2",
    )
    .bind(request.project_id)
    .bind(request.proposal_id)
    .bind(if matches!(request.decision, ProposalDecision::Reject) {
        "rejected"
    } else {
        "accepted"
    })
    .bind(&request.actor_kind)
    .bind(&request.actor_id)
    .bind(&request.reason)
    .execute(&mut *tx)
    .await?;
    if matches!(request.decision, ProposalDecision::Reject) {
        sqlx::query(
            "INSERT INTO dedupe_resolution_events
             (id,project_id,record_id,prior_report_id,resolved_report_id,action,reason,actor_kind,actor_id,proposal_id,reverted_event_id)
             VALUES ($1,$2,$3,$4,NULL,$5,$6,$7,$8,$9,NULL)",
        )
        .bind(Uuid::new_v4())
        .bind(request.project_id)
        .bind(record_id)
        .bind(prior_report_id)
        .bind(action)
        .bind(&request.reason)
        .bind(&request.actor_kind)
        .bind(&request.actor_id)
        .bind(request.proposal_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(ResolutionResult {
        record_id,
        prior_report_id,
        resolved_report_id,
        action: request.decision.as_str().to_owned(),
    })
}

async fn supersede_sibling_proposals(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    record_id: Uuid,
    selected_proposal_id: Uuid,
    reason: &str,
    actor_kind: &str,
    actor_id: &str,
) -> Result<(), DedupeError> {
    sqlx::query(
        "UPDATE dedupe_proposals
         SET status='rejected',
             revision=revision+1,
             reviewer_kind=$4,
             reviewer_id=$5,
             decided_at=now(),
             decision_reason=$6,
             metadata=metadata || jsonb_build_object(
                 'action','superseded',
                 'superseded_by',$3::text,
                 'superseded_reason',$6::text
             ),
             updated_at=now()
         WHERE project_id=$1 AND record_id=$2 AND id<>$3 AND status='pending'",
    )
    .bind(project_id)
    .bind(record_id)
    .bind(selected_proposal_id)
    .bind(actor_kind)
    .bind(actor_id)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn resolve_record(
    pool: &PgPool,
    request: ResolveRecordCommand,
) -> Result<ResolutionResult, DedupeError> {
    request
        .validate()
        .map_err(|error| DedupeError::InvalidCommand(error.to_string()))?;
    validate_actor(&request.actor_kind, &request.actor_id)?;
    let project_id = request.project_id.as_uuid();
    let record_id = request.record_id.as_uuid();
    let mut tx = pool.begin().await?;
    ensure_project(&mut tx, project_id).await?;
    lock_project(&mut tx, project_id).await?;
    let prior_report_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT report_id FROM records WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(record_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    if prior_report_id.is_none() {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM records WHERE project_id=$1 AND id=$2)",
        )
        .bind(project_id)
        .bind(record_id)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(DedupeError::RecordNotFound);
        }
    }
    let mut proposal_kind = None;
    if let Some(proposal_id) = request.proposal_id
        && !matches!(request.action, RecordResolutionAction::Revert)
    {
        let proposal = sqlx::query(
            "SELECT record_id,proposal_kind,status FROM dedupe_proposals
             WHERE project_id=$1 AND id=$2 FOR UPDATE",
        )
        .bind(project_id)
        .bind(proposal_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DedupeError::ProposalNotFound)?;
        if proposal.get::<Uuid, _>("record_id") != record_id {
            return Err(DedupeError::ProposalNotFound);
        }
        if proposal.get::<String, _>("status") != "pending" {
            return Err(DedupeError::ProposalNotPending);
        }
        proposal_kind = Some(proposal.get::<String, _>("proposal_kind"));
        if matches!(request.action, RecordResolutionAction::Create)
            && proposal_kind.as_deref() == Some("conflict")
        {
            return Err(DedupeError::ConflictCreateNew);
        }
    }

    let resolved_report_id = match request.action {
        RecordResolutionAction::Create => {
            let source = load_source_record(&mut tx, project_id, record_id).await?;
            lock_record_identifiers(&mut tx, record_id).await?;
            let identifiers = record_identifiers(&mut tx, record_id).await?;
            if !matched_report_ids(&mut tx, &identifiers).await?.is_empty() {
                return Err(DedupeError::IdentifierConflict);
            }
            let report_id = create_report(
                &mut tx,
                &source,
                source.title.as_deref().map(normalize_bibliography_title),
            )
            .await?;
            link_record(
                &mut tx,
                ResolutionLink {
                    project_id,
                    record_id,
                    prior_report_id,
                    report_id,
                    action: "create_new",
                    reason: &request.reason,
                    actor_kind: &request.actor_kind,
                    actor_id: &request.actor_id,
                    proposal_id: request.proposal_id,
                },
            )
            .await?;
            Some(report_id)
        }
        RecordResolutionAction::Link | RecordResolutionAction::Reassign => {
            let report_id = request
                .report_id
                .ok_or_else(|| {
                    DedupeError::InvalidCommand("link/reassign requires report_id".to_owned())
                })?
                .as_uuid();
            ensure_report_membership(&mut tx, project_id, report_id).await?;
            let source = load_source_record(&mut tx, project_id, record_id).await?;
            lock_record_identifiers(&mut tx, record_id).await?;
            attach_record_identifiers(
                &mut tx,
                &source,
                report_id,
                proposal_kind.as_deref() == Some("conflict"),
            )
            .await?;
            link_record(
                &mut tx,
                ResolutionLink {
                    project_id,
                    record_id,
                    prior_report_id,
                    report_id,
                    action: request.action.as_str(),
                    reason: &request.reason,
                    actor_kind: &request.actor_kind,
                    actor_id: &request.actor_id,
                    proposal_id: request.proposal_id,
                },
            )
            .await?;
            Some(report_id)
        }
        RecordResolutionAction::Revert => {
            let latest = sqlx::query(
                "SELECT e.id,e.prior_report_id,e.resolved_report_id
                 FROM dedupe_resolution_events e
                 WHERE e.project_id=$1 AND e.record_id=$2
                   AND e.action NOT IN ('revert','reject_proposal')
                   AND NOT EXISTS (
                     SELECT 1 FROM dedupe_resolution_events undo
                     WHERE undo.project_id=e.project_id
                       AND undo.record_id=e.record_id
                       AND undo.reverted_event_id=e.id
                   )
                 ORDER BY e.created_at DESC,e.id DESC
                 LIMIT 1
                 FOR UPDATE",
            )
            .bind(project_id)
            .bind(record_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(DedupeError::RevertConflict)?;
            let latest_event_id: Uuid = latest.get("id");
            let latest_resolved_report_id: Option<Uuid> = latest.get("resolved_report_id");
            if latest_resolved_report_id != prior_report_id {
                return Err(DedupeError::RevertConflict);
            }
            let resolved_report_id: Option<Uuid> = latest.get("prior_report_id");
            if let Some(report_id) = resolved_report_id {
                ensure_report_membership(&mut tx, project_id, report_id).await?;
            }
            sqlx::query("UPDATE records SET report_id=$3 WHERE project_id=$1 AND id=$2")
                .bind(project_id)
                .bind(record_id)
                .bind(resolved_report_id)
                .execute(&mut *tx)
                .await?;
            insert_resolution_event(
                &mut tx,
                ResolutionEvent {
                    project_id,
                    record_id,
                    prior_report_id,
                    resolved_report_id,
                    action: "revert",
                    reason: &request.reason,
                    actor_kind: &request.actor_kind,
                    actor_id: &request.actor_id,
                    proposal_id: request.proposal_id,
                    reverted_event_id: Some(latest_event_id),
                },
            )
            .await?;
            resolved_report_id
        }
    };
    if !matches!(request.action, RecordResolutionAction::Revert)
        && let Some(proposal_id) = request.proposal_id
    {
        supersede_sibling_proposals(
            &mut tx,
            project_id,
            record_id,
            proposal_id,
            &request.reason,
            &request.actor_kind,
            &request.actor_id,
        )
        .await?;
        sqlx::query(
            "UPDATE dedupe_proposals
             SET status='accepted',revision=revision+1,reviewer_kind=$4,reviewer_id=$5,
                 decided_at=now(),decision_reason=$6,updated_at=now()
             WHERE project_id=$1 AND id=$2 AND record_id=$3 AND status='pending'",
        )
        .bind(project_id)
        .bind(proposal_id)
        .bind(record_id)
        .bind(&request.actor_kind)
        .bind(&request.actor_id)
        .bind(&request.reason)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(ResolutionResult {
        record_id,
        prior_report_id,
        resolved_report_id,
        action: request.action.as_str().to_owned(),
    })
}

async fn load_source_record(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    record_id: Uuid,
) -> Result<SourceRecord, DedupeError> {
    sqlx::query(
        "SELECT id,title,abstract_text,publication_year,journal,authors,source_identifiers,raw
         FROM records WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(project_id)
    .bind(record_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(|row| SourceRecord {
        id: row.get("id"),
        title: row.get("title"),
        abstract_text: row.get("abstract_text"),
        publication_year: row.get("publication_year"),
        journal: row.get("journal"),
        authors: row.get("authors"),
        source_identifiers: row.get("source_identifiers"),
        raw: row.get("raw"),
    })
    .ok_or(DedupeError::RecordNotFound)
}

async fn ensure_report_membership(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<(), DedupeError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(DedupeError::ReportNotInProject)
    }
}

async fn insert_resolution_event(
    tx: &mut Transaction<'_, Postgres>,
    event: ResolutionEvent<'_>,
) -> Result<(), DedupeError> {
    sqlx::query(
        "INSERT INTO dedupe_resolution_events
         (id,project_id,record_id,prior_report_id,resolved_report_id,action,reason,actor_kind,actor_id,proposal_id,reverted_event_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(Uuid::new_v4())
    .bind(event.project_id)
    .bind(event.record_id)
    .bind(event.prior_report_id)
    .bind(event.resolved_report_id)
    .bind(event.action)
    .bind(event.reason)
        .bind(event.actor_kind)
        .bind(event.actor_id)
        .bind(event.proposal_id)
        .bind(event.reverted_event_id)
        .execute(&mut **tx)
    .await?;
    Ok(())
}

fn validate_actor(kind: &str, id: &str) -> Result<(), DedupeError> {
    if !matches!(kind, "user" | "automation" | "system") || id.trim().is_empty() {
        return Err(DedupeError::InvalidCommand(
            "actor_kind must be user, automation, or system and actor_id must not be empty"
                .to_owned(),
        ));
    }
    Ok(())
}

fn proposal_from_row(row: sqlx::postgres::PgRow) -> DedupeProposal {
    DedupeProposal {
        id: row.get("id"),
        project_id: row.get("project_id"),
        record_id: row.get("record_id"),
        candidate_report_id: row.get("candidate_report_id"),
        proposal_kind: row.get("proposal_kind"),
        source_title: row.get("source_title"),
        source_abstract: row.get("source_abstract"),
        source_year: row.get("source_year"),
        source_authors: row.get("source_authors"),
        source_identifiers: row.get("source_identifiers"),
        candidate_title: row.get("candidate_title"),
        candidate_year: row.get("candidate_year"),
        candidate_authors: row
            .try_get("candidate_authors")
            .unwrap_or(Value::Array(Vec::new())),
        candidate_identifiers: row.get("candidate_identifiers"),
        title_similarity: row.get("title_similarity"),
        year_match: row.get("year_match"),
        first_author_similarity: row.get("first_author_similarity"),
        exact_identifier_match: row.get("exact_identifier_match"),
        conflicting_identifier: row.get("conflicting_identifier"),
        score: row.get("score"),
        metadata: row.get("metadata"),
        status: row.get("status"),
        revision: row.get("revision"),
        reviewer_kind: row.get("reviewer_kind"),
        reviewer_id: row.get("reviewer_id"),
        decided_at: row.get("decided_at"),
        decision_reason: row.get("decision_reason"),
        created_at: row.get("created_at"),
    }
}
