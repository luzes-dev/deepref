# Object storage lifecycle and workflow cutover

Document and workflow downloads remain server mediated. Route authorization resolves the project before the API reads the persisted object key. Buckets must remain private; no signed-URL endpoint is introduced.

During rolling overlap, N writes legacy-readable `documents/{uuid}` and `projects/{project}/workflow-runs/{run}/files/{uuid}` keys, and reads both legacy and tenant-prefixed layouts. Migrations 0146–0147 add and validate a separately named wider CHECK while retaining the released restrictive CHECK. Neither the key layout nor compatibility bytes change during ordinary upgrade; rollback to N-1 remains possible.

After every legacy-only key parser has actually drained, activate the new layout explicitly:

```bash
deepref-server object-storage activate-tenant-keys --confirm-drained all-legacy-key-parsers-drained
```

Activation obtains the document table lock without waiting, locks the singleton rollout row, verifies both exact known validated CHECK expressions, and atomically drops only the restrictive legacy CHECK and persists activation plus the `tenant-object-keys-v1` parser rollback floor. Wrong tokens, unexpected or unvalidated constraints, a producer holding the table, or a failed state write leave the old CHECK and flag unchanged. New upload intents reject tenant-prefixed project keys before activation. A legacy upload already underway may finish after activation. Once tenant keys exist, rollback binaries must understand both layouts; prevent drained legacy-only parsers from restarting through the deployment controller. This parser floor is separate from the `cleanup_objects` queue-kind fence and from the workflow-content cutover below.

Activated document attempts use `workspaces/{workspace}/projects/{project}/documents/{uuid}`. Activated workflow keys use `workspaces/{workspace}/projects/{project}/workflow-runs/{run}/files/{uuid}`. Names never come from filenames. Existing objects remain readable without renaming. Account exports independently use `account-exports/{user}/{export}` and require the privacy feature's user-only authorization; the account-export seam is unchanged by project-key activation.

The schema inventory found only one file-body `bytea` column: `workflow_files.content`. The other `bytea` columns are fixed-size session/security digests and remain relational. PDFs already store object references; parsed document blocks and normalized scientific text stay in PostgreSQL. No second object-storage layer is introduced.

## Durable ownership and cleanup

`object_lifecycle` survives tenant deletion. The caller commits a pending intent before upload, then holds the returned transaction's registry row lock throughout upload and metadata/reference commit:

```rust,ignore
let key = ObjectKey::account_export(user.as_uuid(), attempt_id);
let mut tx = begin_object_upload(pool, ObjectOwner::Account(user), &key).await?;
let stored = store.for_upload(key.clone()).put_stream(zip_stream).await?;
retain_object_reference(&mut tx, ObjectOwner::Account(user), &key, export_id, expires_at).await?;
// Publish the export's account-owned download record in this same transaction.
tx.commit().await?;
```

A key is immutable and unique per attempt. A failed/ambiguous commit leaves durable intent; callers must never delete the object eagerly after a failed commit. Document/workflow triggers attach live references, reject tombstoned/foreign-tenant keys, and schedule obsolete references atomically on deletion or replacement. Project/workspace cascades therefore retain cleanup intent after metadata disappears.

The worker reconciler schedules platform `cleanup_objects` work. A database capability fence parks this new kind outside old workers' queued-before-LIMIT candidate sets, including their expired-lease recovery. New workers declare their decoder set using the existing claim protocol. Heartbeats do not authorize decoding.

Cleanup locks the registry row, rechecks all live document/workflow rows and retained references, then commits permanent retirement before any external deletion. It reacquires the row lock for idempotent deletion and records completion separately. New references reject retired keys even if the worker crashes after deleting bytes but before acknowledging completion. Missing bytes are success; storage errors leave intent for retry. A retained source reference keeps bytes after source metadata is removed. The privacy workflow must serialize publish/release using its durable export record lock. Retain multiple source keys in sorted key order inside one transaction; the account-artifact key sorts before source document/workflow keys. `release_object_reference` releases all keys retained under one export/download UUID transactionally. Expired references no longer protect bytes. Download owners must retain every source/artifact until their advertised access expires; a server stream still needs a bounded deadline within that retention window.

The 24-hour grace is operational cleanup grace after metadata or download ownership is released. It does not authorize research/account deletion before the 30-day business-policy window. Privacy owns policy scheduling and account export selection. This module owns physical blob deletion.

The additive migration inventories currently referenced document/workflow objects. Legacy unreferenced objects created before durable intent was deployed can be adopted after all old uploaders stop:

```bash
deepref-server object-storage reconcile-orphans --confirm-drained all-legacy-uploaders-drained
```

This performs a complete inventory bounded at 10,000 objects, failing before writing any intent when the limit is exceeded. It preserves unknown key shapes, objects newer than 24 hours, already registered keys and live references; older recognized orphan keys receive durable intent with a further 24-hour grace. A partial inventory never establishes safe deletion. For a bucket beyond the CLI bound, the application function accepts a bound up to 100,000 objects; larger inventories need a reviewed paginated reconciliation workflow before legacy cleanup. Incomplete multipart uploads require a provider lifecycle rule after the upload timeout/drain window. Production lifecycle rules must never expire live prefixes indiscriminately.

## Workflow storage phases

Ordinary migration preserves released `workflow_files.content NOT NULL`, every compatibility byte, and dual-write behavior. Backfill writes a unique attempt key, reads it back, verifies size/SHA-256, and only then attaches metadata. New readers prefer verified objects and preserve compatibility fallback during overlap.

After verified backfill and a maintenance window in which every old reader and writer has actually stopped:

```bash
deepref-server workflow-storage object-only --confirm-drained all-legacy-readers-and-writers-drained
```

The command obtains an exclusive table lock without waiting, verifies every row's object bytes against recorded size/hash, and atomically relaxes `content NOT NULL` and switches new writers to object-only. Missing references, missing objects or hash mismatch abort the transaction. A database guard rejects legacy writers after cutover. Drain attestations are operator assertions, not proof derived from heartbeats. Prevent old binaries from restarting through the deployment controller before running the command.

In a subsequent release, after all binaries that reference `content` have drained:

```bash
deepref-server workflow-storage contract --confirm-drained all-content-dependent-binaries-drained-subsequent-release
```

The command repeats live verification under the exclusive lock and drops `content` only in this explicit transaction. Do not run this as an ordinary migration or immediately during initial rollout. After this point rollback binaries must understand object-only storage; loss/corruption of objects fails closed and requires object recovery. Backups/restore procedures must preserve object versions and these cutover states together.

## Contract verification

`bash scripts/ci/test-object-storage.sh` starts a pinned, disposable Adobe S3Mock TLS fixture, trusts only that loopback fixture certificate, runs the same multipart/size/hash/read/head/delete/abort contract used for filesystem storage, and removes only the container it created. `integration.object-storage` is a mandatory probe in the existing integration diagnostic gate. The fixture tests S3 protocol compatibility; it does not prove cloud IAM, provider signature enforcement or real bucket policy.

## Deployment acceptance still required

Configure the private S3-compatible bucket in the approved São Paulo region; deploy N with default legacy overlap; drain legacy key-only parsers and explicitly activate tenant keys before tenant-prefixed writes; drain legacy uploaders/readers/writers before destructive maintenance; establish old-orphan and incomplete-multipart provider policies; verify restore/version retention and user-facing artifact retention in the integrated privacy release. Local contract tests do not establish those deployment facts.
