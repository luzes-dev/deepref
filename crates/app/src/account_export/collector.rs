//! Explicit snapshot inventory. Every bound is account-wide, not per project.
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use deepref_documents::ObjectKey;
use deepref_domain::{ProjectId, UserId, billing::BillingProjection};
use futures::TryStreamExt;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub const MAX_WORKSPACES: usize = 50;
pub const MAX_ROWS: usize = 20_000;
pub const MAX_FILES: usize = 2_000;
pub const MAX_JSON_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ROW_BYTES: i32 = 1024 * 1024;
pub const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;

pub enum Source {
    Object { key: ObjectKey, project: ProjectId },
    Inline(PathBuf),
}
pub struct SourceBlob {
    pub source: Source,
    pub byte_size: u64,
    pub sha256: String,
    pub archive_path: String,
}
pub struct Inventory {
    pub json_files: Vec<(String, PathBuf)>,
    pub blobs: Vec<SourceBlob>,
    pub workspaces: Vec<Uuid>,
}
#[derive(Default)]
struct Budget {
    rows: usize,
    json_bytes: usize,
    source_bytes: u64,
}
impl Budget {
    fn json(&mut self, bytes: usize) -> anyhow::Result<()> {
        self.rows += 1;
        self.json_bytes += bytes;
        ensure!(self.rows <= MAX_ROWS, "export row budget exceeded");
        ensure!(
            self.json_bytes <= MAX_JSON_BYTES,
            "export JSON budget exceeded"
        );
        Ok(())
    }
    fn source(&mut self, bytes: i64, count: usize) -> anyhow::Result<u64> {
        let bytes = u64::try_from(bytes).context("negative source length")?;
        self.source_bytes = self
            .source_bytes
            .checked_add(bytes)
            .context("source size overflow")?;
        ensure!(
            count < MAX_FILES && self.source_bytes <= MAX_SOURCE_BYTES,
            "export source budget exceeded"
        );
        Ok(bytes)
    }
}

macro_rules! section_query {
    ($query:literal) => {
        concat!("SELECT CASE WHEN octet_length(row_to_json(q)::text) <= $2 THEN row_to_json(q)::text END AS body FROM (", $query, ") q LIMIT $3")
    };
}

/// The subquery is a static allowlist. PostgreSQL checks the encoded row size
/// before sending it; no aggregate JSON value or unbounded fetch_all is used.
async fn json_section(
    tx: &mut Transaction<'_, Postgres>,
    dir: &Path,
    name: &str,
    query: &'static str,
    ids: &[Uuid],
    budget: &mut Budget,
) -> anyhow::Result<(String, PathBuf)> {
    let path = dir.join(name);
    let mut file = std::fs::File::create(&path)?;
    file.write_all(b"[")?;
    let mut rows = sqlx::query(query)
        .bind(ids)
        .bind(MAX_ROW_BYTES)
        .bind(i64::try_from(MAX_ROWS + 1)?)
        .fetch(&mut **tx);
    let mut first = true;
    while let Some(row) = rows.try_next().await? {
        let body: Option<String> = row.try_get("body")?;
        let body = body.context("export JSON row exceeds budget")?;
        budget.json(body.len() + 1)?;
        if !first {
            file.write_all(b",")?;
        }
        first = false;
        file.write_all(body.as_bytes())?;
    }
    file.write_all(b"]")?;
    Ok((name.to_owned(), path))
}

pub async fn collect(
    tx: &mut Transaction<'_, Postgres>,
    user: UserId,
    workspaces: Vec<Uuid>,
    dir: &Path,
) -> anyhow::Result<Inventory> {
    let mut budget = Budget::default();
    let mut json_files = vec![
        json_section(
            tx,
            dir,
            "profile.json",
            section_query!(
                "SELECT id,primary_email,display_name,status,created_at FROM users WHERE id=ANY($1)"
            ),
            &[user.as_uuid()],
            &mut budget,
        )
        .await?,
    ];
    // These columns are taken from the released relational schemas. Raw
    // connector/provider payloads, workflow secrets and audit are not selected.
    let sections = [
        (
            "workspaces.json",
            section_query!(
                "SELECT id,kind,name,slug,created_at FROM workspaces WHERE id=ANY($1) ORDER BY id"
            ),
        ),
        (
            "memberships.json",
            section_query!(
                "SELECT workspace_id,user_id,role,status,created_at FROM workspace_memberships WHERE workspace_id=ANY($1) ORDER BY workspace_id,user_id"
            ),
        ),
        (
            "projects.json",
            section_query!(
                "SELECT id,workspace_id,name,description,created_at FROM projects WHERE workspace_id=ANY($1) ORDER BY id"
            ),
        ),
        (
            "protocols.json",
            section_query!(
                "SELECT v.id,v.project_id,v.version,v.name,v.status,v.framework_kind,v.framework_fields,v.objective,v.question,v.criteria,v.published_at,v.created_at FROM protocol_versions v JOIN projects p ON p.id=v.project_id WHERE p.workspace_id=ANY($1) ORDER BY v.id"
            ),
        ),
        (
            "criteria.json",
            section_query!(
                "SELECT c.id,c.protocol_version_id,c.criterion_type,c.label,c.description,c.ordinal,c.stage,c.dimension FROM eligibility_criteria c JOIN protocol_versions v ON v.id=c.protocol_version_id JOIN projects p ON p.id=v.project_id WHERE p.workspace_id=ANY($1) ORDER BY c.id"
            ),
        ),
        (
            "records.json",
            section_query!(
                "SELECT r.id,r.project_id,r.report_id,r.source,r.title,r.abstract_text,r.publication_year,r.created_at FROM records r JOIN projects p ON p.id=r.project_id WHERE p.workspace_id=ANY($1) ORDER BY r.id"
            ),
        ),
        (
            "reports.json",
            section_query!(
                "SELECT r.id,r.title,r.abstract_text,r.publication_year,r.journal,r.created_at FROM reports r WHERE EXISTS(SELECT 1 FROM project_reports pr JOIN projects p ON p.id=pr.project_id WHERE pr.report_id=r.id AND p.workspace_id=ANY($1)) ORDER BY r.id"
            ),
        ),
        (
            "studies.json",
            section_query!(
                "SELECT s.id,s.project_id,s.title,s.design,s.created_at FROM studies s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
            ),
        ),
        (
            "screening_events.json",
            section_query!(
                "SELECT s.id,s.project_id,s.report_id,s.stage,s.decision,s.exclusion_reason_id,s.notes,s.protocol_version_id,s.actor_kind,s.actor_id,s.created_at FROM screening_events s JOIN projects p ON p.id=s.project_id WHERE p.workspace_id=ANY($1) ORDER BY s.id"
            ),
        ),
        (
            "documents.json",
            section_query!(
                "SELECT d.id,d.project_id,d.report_id,d.mime_type,d.byte_size,d.content_hash,d.original_filename,d.status,d.created_at FROM documents d JOIN projects p ON p.id=d.project_id WHERE p.workspace_id=ANY($1) ORDER BY d.id"
            ),
        ),
        (
            "workflow_files.json",
            section_query!(
                "SELECT f.id,f.project_id,f.run_id,f.node_id,f.name,f.content_type,f.byte_size,f.content_sha256,f.created_at FROM workflow_files f JOIN projects p ON p.id=f.project_id WHERE p.workspace_id=ANY($1) ORDER BY f.id"
            ),
        ),
    ];
    for (name, query) in sections {
        json_files.push(json_section(tx, dir, name, query, &workspaces, &mut budget).await?);
    }
    // Decode then reserialize the domain projection: arbitrary persisted JSON
    // fields and provider references cannot escape through this local view.
    let billing_path = dir.join("billing.json");
    let mut billing_file = std::fs::File::create(&billing_path)?;
    billing_file.write_all(b"[")?;
    let mut first = true;
    let mut billing = sqlx::query("SELECT workspace_id,CASE WHEN octet_length(projection::text)<=$2 THEN projection END AS projection,review_required,provisioning='ambiguous' AS provisioning_ambiguous,reconciled_at FROM billing_accounts WHERE workspace_id=ANY($1) AND provisioning<>'detached' ORDER BY workspace_id,id LIMIT $3")
        .bind(&workspaces).bind(MAX_ROW_BYTES).bind(i64::try_from(MAX_ROWS + 1)?).fetch(&mut **tx);
    while let Some(row) = billing.try_next().await? {
        let raw: Option<serde_json::Value> = row.try_get("projection")?;
        let projection: BillingProjection =
            serde_json::from_value(raw.context("billing projection exceeds budget")?)?;
        let value = serde_json::json!({"workspace_id":row.try_get::<Uuid,_>("workspace_id")?,"projection":projection,
            "review_required":row.try_get::<bool,_>("review_required")?,"provisioning_ambiguous":row.try_get::<bool,_>("provisioning_ambiguous")?,
            "reconciled_at":row.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("reconciled_at")?});
        let bytes = serde_json::to_vec(&value)?;
        budget.json(bytes.len())?;
        if !first {
            billing_file.write_all(b",")?;
        }
        first = false;
        billing_file.write_all(&bytes)?;
    }
    drop(billing);
    let mut free = sqlx::query("SELECT workspace_id,enrolled_at,CASE WHEN octet_length(entitlements::text)<=$2 THEN entitlements END AS entitlements FROM billing_free_enrollments WHERE workspace_id=ANY($1) ORDER BY workspace_id LIMIT $3")
        .bind(&workspaces).bind(MAX_ROW_BYTES).bind(i64::try_from(MAX_ROWS + 1)?).fetch(&mut **tx);
    while let Some(row) = free.try_next().await? {
        let raw: Option<serde_json::Value> = row.try_get("entitlements")?;
        let projection = BillingProjection::Free {
            enrolled_at: row.try_get("enrolled_at")?,
            entitlements: serde_json::from_value(raw.context("entitlements exceed budget")?)?,
        };
        let bytes = serde_json::to_vec(
            &serde_json::json!({"workspace_id":row.try_get::<Uuid,_>("workspace_id")?,"projection":projection}),
        )?;
        budget.json(bytes.len())?;
        if !first {
            billing_file.write_all(b",")?;
        }
        first = false;
        billing_file.write_all(&bytes)?;
    }
    drop(free);
    billing_file.write_all(b"]")?;
    json_files.push(("billing.json".to_owned(), billing_path));

    let mut blobs = Vec::new();
    let mut docs = sqlx::query("SELECT d.id,d.project_id,CASE WHEN octet_length(d.object_key)<=512 THEN d.object_key END AS object_key,d.byte_size,d.content_hash FROM documents d JOIN projects p ON p.id=d.project_id WHERE p.workspace_id=ANY($1) AND d.object_key IS NOT NULL ORDER BY d.id LIMIT $2")
        .bind(&workspaces).bind(i64::try_from(MAX_FILES + 1)?).fetch(&mut **tx);
    while let Some(row) = docs.try_next().await? {
        let id: Uuid = row.try_get("id")?;
        let hash: String = row.try_get("content_hash")?;
        let hash = hash.strip_prefix("sha256:").unwrap_or(&hash).to_owned();
        validate_hash(&hash)?;
        blobs.push(SourceBlob {
            byte_size: budget.source(row.try_get("byte_size")?, blobs.len())?,
            sha256: hash,
            archive_path: format!("documents/{id}"),
            source: Source::Object {
                key: ObjectKey::parse(&row.try_get::<String, _>("object_key")?)?,
                project: ProjectId::new(row.try_get("project_id")?),
            },
        });
    }
    drop(docs);
    let has_inline: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='workflow_files' AND column_name='content')").fetch_one(&mut **tx).await?;
    let query = if has_inline {
        "SELECT f.id,f.project_id,CASE WHEN octet_length(f.storage_key)<=512 THEN f.storage_key END AS storage_key,(f.storage_key IS NULL OR octet_length(f.storage_key)<=512) AS key_size_valid,coalesce(f.byte_size,octet_length(f.content)::bigint) AS byte_size,coalesce(f.content_sha256,encode(digest(f.content,'sha256'),'hex')) AS sha256 FROM workflow_files f JOIN projects p ON p.id=f.project_id WHERE p.workspace_id=ANY($1) ORDER BY f.id LIMIT $2"
    } else {
        "SELECT f.id,f.project_id,CASE WHEN octet_length(f.storage_key)<=512 THEN f.storage_key END AS storage_key,(f.storage_key IS NULL OR octet_length(f.storage_key)<=512) AS key_size_valid,f.byte_size,f.content_sha256 AS sha256 FROM workflow_files f JOIN projects p ON p.id=f.project_id WHERE p.workspace_id=ANY($1) ORDER BY f.id LIMIT $2"
    };
    // A bounded metadata list permits subsequent chunk reads on the same sole
    // connection. No file body is loaded by this query.
    let files = sqlx::query(query)
        .bind(&workspaces)
        .bind(i64::try_from(MAX_FILES + 1)?)
        .fetch_all(&mut **tx)
        .await?;
    for row in files {
        ensure!(
            row.try_get::<bool, _>("key_size_valid")?,
            "workflow object key exceeds budget"
        );
        let id: Uuid = row.try_get("id")?;
        let size = budget.source(row.try_get("byte_size")?, blobs.len())?;
        let hash: String = row.try_get("sha256")?;
        validate_hash(&hash)?;
        let source = match row.try_get::<Option<String>, _>("storage_key")? {
            Some(key) => Source::Object {
                key: ObjectKey::parse(&key)?,
                project: ProjectId::new(row.try_get("project_id")?),
            },
            None => {
                ensure!(has_inline, "workflow file has no body");
                let path = dir.join(id.to_string());
                let mut output = std::fs::File::create(&path)?;
                let mut offset = 0u64;
                while offset < size {
                    let chunk: Vec<u8> = sqlx::query_scalar("SELECT substring(content FROM $2 FOR 65536) FROM workflow_files WHERE id=$1")
                        .bind(id).bind(i32::try_from(offset + 1)?).fetch_one(&mut **tx).await?;
                    ensure!(!chunk.is_empty(), "inline workflow body ended early");
                    offset += u64::try_from(chunk.len())?;
                    ensure!(offset <= size, "inline workflow length mismatch");
                    output.write_all(&chunk)?;
                }
                Source::Inline(path)
            }
        };
        blobs.push(SourceBlob {
            source,
            byte_size: size,
            sha256: hash,
            archive_path: format!("workflow_files/{id}"),
        });
    }
    Ok(Inventory {
        json_files,
        blobs,
        workspaces,
    })
}

fn validate_hash(hash: &str) -> anyhow::Result<()> {
    ensure!(
        hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid source SHA256"
    );
    Ok(())
}
