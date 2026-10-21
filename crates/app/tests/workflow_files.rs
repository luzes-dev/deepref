#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! `workflow_files` during the object-storage overlap: writers store the
//! object and still the bytes, readers prefer a verified object, and the
//! backfill moves older rows over.

use std::path::{Path, PathBuf};

use deepref_app::{
    NewWorkflowFile, WorkflowFileFallback, WorkflowFileSource, backfill_workflow_files,
    create_workflow_file, get_workflow_file, schedule_workflow_file_backfills,
};
use deepref_documents::{DocumentStore, ObjectKey, content_sha256};
use deepref_domain::ProjectId;
use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

struct Fixture {
    project: Uuid,
    run: Uuid,
}

impl Fixture {
    fn project_id(&self) -> ProjectId {
        ProjectId::new(self.project)
    }
}

async fn fixture(pool: &PgPool) -> Fixture {
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project)
        .bind(format!("workflow files {project}"))
        .execute(pool)
        .await
        .expect("project");
    let run = new_run(pool, project).await;
    Fixture { project, run }
}

async fn new_run(pool: &PgPool, project: Uuid) -> Uuid {
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO durable_runs (id, project_id, kind, actor_kind, actor_id, idempotency_key, status)
         VALUES ($1,$2,'workflow','user','fixture',$3,'queued')",
    )
    .bind(run)
    .bind(project)
    .bind(run.to_string())
    .execute(pool)
    .await
    .expect("run");
    run
}

/// A row as a binary that predates the object columns writes it.
async fn legacy_file(pool: &PgPool, fixture: &Fixture, content: &[u8]) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow_files (project_id, run_id, node_id, name, content_type, content)
         VALUES ($1,$2,'export','legacy.csv','text/csv',$3) RETURNING id",
    )
    .bind(fixture.project)
    .bind(fixture.run)
    .bind(content)
    .fetch_one(pool)
    .await
    .expect("legacy row")
}

fn new_file<'a>(
    project_id: Uuid,
    run_id: Uuid,
    name: &'a str,
    content: &'a [u8],
) -> NewWorkflowFile<'a> {
    NewWorkflowFile {
        project_id,
        run_id,
        node_id: "export",
        name,
        content_type: "text/csv",
        content,
    }
}

async fn create(pool: &PgPool, store: &DocumentStore, fixture: &Fixture, content: &[u8]) -> Uuid {
    create_workflow_file(
        pool,
        store,
        new_file(fixture.project, fixture.run, "result.csv", content),
    )
    .await
    .expect("file is stored")
}

async fn reference(pool: &PgPool, file: Uuid) -> (Option<String>, Option<String>, Option<i64>) {
    let row = sqlx::query(
        "SELECT storage_key, content_sha256, byte_size FROM workflow_files WHERE id=$1",
    )
    .bind(file)
    .fetch_one(pool)
    .await
    .expect("row");
    (
        row.get("storage_key"),
        row.get("content_sha256"),
        row.get("byte_size"),
    )
}

fn key_of(fixture: &Fixture, file: Uuid) -> ObjectKey {
    ObjectKey::workflow_file(fixture.project, fixture.run, file)
}

fn local_store() -> (DocumentStore, PathBuf) {
    let root = std::env::temp_dir().join(format!("deepref-workflow-files-{}", Uuid::new_v4()));
    let config = deepref_documents::StoreConfig::local(&root).expect("local store");
    (DocumentStore::new(config, 1024 * 1024), root)
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).expect("readable directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

#[tokio::test]
async fn dual_write_populates_the_object_the_reference_and_the_database_copy() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let file = create(&pool, &store, &fixture, b"id,title\n1,A\n").await;

    let (key, sha256, size) = reference(&pool, file).await;
    let key = key.expect("storage key is recorded");
    assert_eq!(key, key_of(&fixture, file).as_str());
    assert_eq!(sha256.unwrap(), content_sha256(b"id,title\n1,A\n"));
    assert_eq!(size, Some(13));
    let object = store
        .get_object(&ObjectKey::parse(&key).unwrap(), 1024)
        .await
        .expect("object exists");
    assert_eq!(object.as_ref(), b"id,title\n1,A\n");
    let content: Vec<u8> = sqlx::query_scalar("SELECT content FROM workflow_files WHERE id=$1")
        .bind(file)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        content, b"id,title\n1,A\n",
        "old readers still find the bytes"
    );
}

#[tokio::test]
async fn reader_prefers_the_verified_object_over_the_database_copy() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let file = create(&pool, &store, &fixture, b"from the object").await;
    sqlx::query("UPDATE workflow_files SET content=$2 WHERE id=$1")
        .bind(file)
        .bind(b"a different database copy".as_slice())
        .execute(&pool)
        .await
        .unwrap();

    let read = get_workflow_file(&pool, &store, fixture.project_id(), file)
        .await
        .expect("read");
    assert_eq!(read.content, b"from the object");
    assert_eq!(read.source, WorkflowFileSource::Object);
    assert_eq!(read.name, "result.csv");
    assert_eq!(read.content_type, "text/csv");
}

#[tokio::test]
async fn a_row_without_a_storage_key_is_read_from_the_database() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let file = legacy_file(&pool, &fixture, b"legacy bytes").await;

    let read = get_workflow_file(&pool, &store, fixture.project_id(), file)
        .await
        .expect("read");
    assert_eq!(read.content, b"legacy bytes");
    assert_eq!(read.source, WorkflowFileSource::Database);
}

#[tokio::test]
async fn an_object_that_fails_hash_verification_is_reported_and_replaced_by_the_database_copy() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let file = create(&pool, &store, &fixture, b"original bytes").await;

    // Same length, different bytes: only the hash can tell.
    store
        .put_object(
            &key_of(&fixture, file),
            b"tampered bytes".as_slice().into(),
            1024,
        )
        .await
        .unwrap();
    let read = get_workflow_file(&pool, &store, fixture.project_id(), file)
        .await
        .expect("read");
    assert_eq!(read.content, b"original bytes");
    assert_eq!(
        read.source,
        WorkflowFileSource::DatabaseFallback(WorkflowFileFallback::IntegrityMismatch)
    );

    // A different length is caught before the object is downloaded.
    store
        .put_object(
            &key_of(&fixture, file),
            b"truncated".as_slice().into(),
            1024,
        )
        .await
        .unwrap();
    let read = get_workflow_file(&pool, &store, fixture.project_id(), file)
        .await
        .expect("read");
    assert_eq!(read.content, b"original bytes");
    assert_eq!(
        read.source,
        WorkflowFileSource::DatabaseFallback(WorkflowFileFallback::IntegrityMismatch)
    );
}

#[tokio::test]
async fn a_missing_object_falls_back_to_the_database_copy() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let file = create(&pool, &store, &fixture, b"still in the database").await;
    store.delete_object(&key_of(&fixture, file)).await.unwrap();

    let read = get_workflow_file(&pool, &store, fixture.project_id(), file)
        .await
        .expect("read");
    assert_eq!(read.content, b"still in the database");
    assert_eq!(
        read.source,
        WorkflowFileSource::DatabaseFallback(WorkflowFileFallback::ObjectMissing)
    );
}

#[tokio::test]
async fn a_file_is_only_readable_through_its_own_project() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let other = self::fixture(&pool).await;
    let file = create(&pool, &store, &fixture, b"private").await;

    assert!(
        get_workflow_file(&pool, &store, other.project_id(), file)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn object_keys_come_from_identifiers_and_never_from_the_file_name() {
    let Some(pool) = database().await else { return };
    let (store, root) = local_store();
    let fixture = fixture(&pool).await;
    let file = create_workflow_file(
        &pool,
        &store,
        new_file(
            fixture.project,
            fixture.run,
            "../../etc/passwd or Résumé.csv",
            b"bytes",
        ),
    )
    .await
    .expect("stored");

    let (key, _, _) = reference(&pool, file).await;
    let key = key.unwrap();
    assert_eq!(
        key,
        format!(
            "projects/{}/workflow-runs/{}/files/{file}",
            fixture.project, fixture.run
        )
    );
    assert!(!key.contains("passwd") && !key.contains("csv") && !key.contains('.'));
    let written = files_under(&root);
    assert_eq!(written.len(), 1);
    assert!(
        written[0].ends_with(&key),
        "{} should end with {key}",
        written[0].display()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_upload_stores_no_row_and_a_failed_insert_removes_the_object() {
    use std::os::unix::fs::PermissionsExt;

    let Some(pool) = database().await else { return };
    let (store, root) = local_store();
    let fixture = fixture(&pool).await;

    // The insert fails after the upload: the run does not exist.
    let result = create_workflow_file(
        &pool,
        &store,
        new_file(fixture.project, Uuid::new_v4(), "result.csv", b"bytes"),
    )
    .await;
    assert!(matches!(
        result,
        Err(deepref_app::WorkflowError::Database(_))
    ));
    assert!(
        files_under(&root).is_empty(),
        "the object of the row that was not stored is removed"
    );

    std::fs::remove_dir_all(&root).unwrap();

    // The upload itself fails: nothing reaches the database.
    let (store, root) = local_store();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = create_workflow_file(
        &pool,
        &store,
        new_file(fixture.project, fixture.run, "result.csv", b"bytes"),
    )
    .await;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        result,
        Err(deepref_app::WorkflowError::ObjectStorage(_))
    ));
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow_files")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_file_over_the_size_limit_is_refused_before_anything_is_stored() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let too_large = vec![0u8; deepref_app::MAX_WORKFLOW_FILE_BYTES + 1];
    let result = create_workflow_file(
        &pool,
        &store,
        new_file(fixture.project, fixture.run, "big.bin", &too_large),
    )
    .await;
    assert!(matches!(
        result,
        Err(deepref_app::WorkflowError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn backfill_moves_older_rows_verifies_them_and_can_be_repeated() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let contents: [&[u8]; 3] = [b"first", b"second file", b""];
    let mut files = Vec::new();
    for content in contents {
        files.push(legacy_file(&pool, &fixture, content).await);
    }

    let first = backfill_workflow_files(&pool, &store, fixture.project_id(), 10)
        .await
        .expect("backfill");
    assert_eq!((first.migrated, first.failed, first.remaining), (3, 0, 0));
    for (file, content) in files.iter().zip(contents) {
        let (key, sha256, size) = reference(&pool, *file).await;
        assert_eq!(key.unwrap(), key_of(&fixture, *file).as_str());
        assert_eq!(sha256.unwrap(), content_sha256(content));
        assert_eq!(size, Some(content.len() as i64));
        let read = get_workflow_file(&pool, &store, fixture.project_id(), *file)
            .await
            .unwrap();
        assert_eq!(read.content, content);
        assert_eq!(read.source, WorkflowFileSource::Object);
    }

    let again = backfill_workflow_files(&pool, &store, fixture.project_id(), 10)
        .await
        .expect("repeat");
    assert_eq!((again.migrated, again.failed, again.remaining), (0, 0, 0));
}

#[tokio::test]
async fn backfill_resumes_where_an_interrupted_run_stopped() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let mut files = Vec::new();
    for index in 0..5u8 {
        files.push(legacy_file(&pool, &fixture, &[index; 8]).await);
    }

    // A run that stops after its batch: the rest is still pending.
    let first = backfill_workflow_files(&pool, &store, fixture.project_id(), 2)
        .await
        .unwrap();
    assert_eq!((first.migrated, first.remaining), (2, 3));

    // A run that died between writing an object and recording it, leaving a
    // damaged object behind under the file's key.
    let mut pending = Vec::new();
    for file in &files {
        if reference(&pool, *file).await.0.is_none() {
            pending.push(*file);
        }
    }
    assert_eq!(pending.len(), 3);
    store
        .put_object(
            &key_of(&fixture, pending[0]),
            b"half-written".as_slice().into(),
            1024,
        )
        .await
        .unwrap();

    let rest = backfill_workflow_files(&pool, &store, fixture.project_id(), 10)
        .await
        .unwrap();
    assert_eq!((rest.migrated, rest.failed, rest.remaining), (3, 0, 0));
    for file in files {
        let read = get_workflow_file(&pool, &store, fixture.project_id(), file)
            .await
            .unwrap();
        assert_eq!(read.source, WorkflowFileSource::Object);
        assert_eq!(read.content.len(), 8);
    }
}

#[tokio::test]
async fn racing_backfills_leave_one_consistent_reference() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let file = legacy_file(&pool, &fixture, b"raced").await;

    let (left, right) = tokio::join!(
        backfill_workflow_files(&pool, &store, fixture.project_id(), 10),
        backfill_workflow_files(&pool, &store, fixture.project_id(), 10),
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left.failed + right.failed, 0);
    let read = get_workflow_file(&pool, &store, fixture.project_id(), file)
        .await
        .unwrap();
    assert_eq!(read.content, b"raced");
    assert_eq!(read.source, WorkflowFileSource::Object);
}

#[cfg(unix)]
#[tokio::test]
async fn backfill_leaves_a_file_it_cannot_store_readable_and_pending() {
    use std::os::unix::fs::PermissionsExt;

    let Some(pool) = database().await else { return };
    let (store, root) = local_store();
    let fixture = fixture(&pool).await;
    let file = legacy_file(&pool, &fixture, b"keep me").await;

    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
    let report = backfill_workflow_files(&pool, &store, fixture.project_id(), 10)
        .await
        .unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        (report.migrated, report.failed, report.remaining),
        (0, 1, 1)
    );
    assert_eq!(reference(&pool, file).await, (None, None, None));
    let read = get_workflow_file(&pool, &store, fixture.project_id(), file)
        .await
        .unwrap();
    assert_eq!(read.content, b"keep me");

    let retry = backfill_workflow_files(&pool, &store, fixture.project_id(), 10)
        .await
        .unwrap();
    assert_eq!((retry.migrated, retry.failed, retry.remaining), (1, 0, 0));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backfill_does_not_touch_other_projects() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let fixture = fixture(&pool).await;
    let other = self::fixture(&pool).await;
    let mine = legacy_file(&pool, &fixture, b"mine").await;
    let theirs = legacy_file(&pool, &other, b"theirs").await;

    backfill_workflow_files(&pool, &store, fixture.project_id(), 10)
        .await
        .unwrap();
    assert!(reference(&pool, mine).await.0.is_some());
    assert!(reference(&pool, theirs).await.0.is_none());
}

#[tokio::test]
async fn scheduling_enqueues_one_job_per_project_with_pending_files_and_is_repeatable() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let busy = fixture(&pool).await;
    let migrated = self::fixture(&pool).await;
    legacy_file(&pool, &busy, b"one").await;
    legacy_file(&pool, &busy, b"two").await;
    create(&pool, &store, &migrated, b"already in object storage").await;

    assert_eq!(
        schedule_workflow_file_backfills(&pool, 100).await.unwrap(),
        1
    );
    assert_eq!(
        schedule_workflow_file_backfills(&pool, 100).await.unwrap(),
        1
    );
    let jobs = sqlx::query(
        "SELECT project_id, kind, payload FROM jobs WHERE kind='backfill_workflow_files'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(jobs.len(), 1, "a repeat pass finds the pending job");
    assert_eq!(jobs[0].get::<Uuid, _>("project_id"), busy.project);
    assert_eq!(
        jobs[0].get::<serde_json::Value, _>("payload"),
        serde_json::json!({ "project_id": busy.project })
    );

    // A change in what is pending changes the job key, so the remainder is
    // scheduled again.
    sqlx::query("DELETE FROM workflow_files WHERE project_id=$1 AND content=$2")
        .bind(busy.project)
        .bind(b"one".as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        schedule_workflow_file_backfills(&pool, 100).await.unwrap(),
        1
    );
    let jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='backfill_workflow_files'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(jobs, 2);

    backfill_workflow_files(&pool, &store, busy.project_id(), 10)
        .await
        .unwrap();
    assert_eq!(
        schedule_workflow_file_backfills(&pool, 100).await.unwrap(),
        0
    );
}
