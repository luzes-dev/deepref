# Calibration deployment and recalibration sequencing

Operator guide for the semantic-contract calibration migration
(`crates/postgres/migrations/0056_semantic_contract_calibration.sql`, PR0 of the
Rig 0.44 + rig-cassette migration). It states the blast radius, the required
migrate → recalibrate → verify sequence, and how to tell an expected
"refused: recalibrate" refusal apart from a real failure.

## Blast radius

After the migration ships, **every calibration bundle stored before it refuses
automation admission until it is recalibrated**. The migration leaves scheme-1
and scheme-2 rows stored for audit but declares they "can never admit
automation under the new recipe"
(`crates/postgres/migrations/0056_semantic_contract_calibration.sql:15-19`).

The mechanism is the scheme gate in
`crates/postgres/src/review_calibration.rs:305-311`: `check_admission` compares
the stored `identity_scheme` against `SEMANTIC_CONTRACT_SCHEME`, which is `3`
(`crates/review/src/contract.rs:49-53`). Any stored scheme other than 3 —
i.e. every row written before PR0 — is refused as
`CalibrationRefusal::IncompatibleIdentityScheme`. A refusal is fail-closed: no
run is scheduled (`crates/postgres/src/review_calibration.rs:67-77`,
`admit_calibration` at `review_calibration.rs:260-288`).

Scope notes:

- Only automation-triggered origins are gated (`AdvisoryTriggered` /
  `AiFirstTriggered` in
  `crates/postgres/src/review_runs.rs:114-120`). Manual scheduling does not go
  through `admit_calibration`.
- A scheme-3 bundle that is `Failed` still refuses as `calibration_failed`
  (`review_calibration.rs:302-304`), independent of the migration.

## Sequence: migrate → recalibrate → verify

1. **Migrate.** Apply `0056_semantic_contract_calibration.sql`. Besides the
   documented refusal semantics, it tightens the
   `review_calibration_bundles_identity_shape_check` constraint to the three
   known shapes (scheme 1 without snapshot, scheme 2 component map, scheme 3
   structured contract) at `0056_semantic_contract_calibration.sql:21-44`.
2. **Recalibrate.** Record one new **scheme-3, `passing`** bundle per
   definition/stage that should admit automation. The bundle identity must come
   from `preview_review_identity` or `preview_screening_identity`, never be
   built by hand (`crates/postgres/src/review_calibration.rs:40-42`), and is
   recorded with `insert_review_calibration_bundle`
   (`review_calibration.rs:192-229`).
   - **Gap, verified:** there is no HTTP endpoint or CLI that records
     calibration bundles. The only callers of
     `insert_review_calibration_bundle` in the tree are tests
     (`crates/postgres/tests/second_review.rs`,
     `crates/postgres/tests/review_runs.rs`); the http-api test suite inserts
     bundles with raw SQL
     (`crates/http-api/tests/exports_postgres.rs:387-417`). Recalibration is
     therefore an out-of-band, expert-adjudicated action today — confirm the
     intended operator path (SQL runbook, admin tool, or a future endpoint)
     with the owning team before announcing the deploy.
3. **Verify (no enable flag exists).** Recovery is automatic: the next
   successful schedule clears the per-stage gate record
   (`crates/postgres/src/second_review.rs:202-205`). Confirm via:
   - `GET /projects/{project_id}/ai/activity/overview`
     (`crates/http-api/src/routes/autonomy.rs:465-491`) — but see the caveat
     below;
   - the absence of rows for the stage in `second_review_gate` (written by
     `record_gate`, `second_review.rs:89-130`), and fresh scheduled counts in
     the sweep (`SecondReviewSweep`, `second_review.rs:47-54`).

**Caveat, verified:** the overview DTO doc comment still advertises
`needs_calibration` / `calibration_stale` states
(`autonomy.rs:446-451`), but `second_review_status` only ever returns
`not_enabled` or `automatic` now — `NeedsCalibration` and `CalibrationStale`
are explicitly "legacy status retained for compatibility; advisory no longer
returns it" (`second_review.rs:23-45`, status function at `75-87`). Do **not**
use the overview endpoint to detect recalibration need. The reliable signals
are the HTTP 409 codes below and the `second_review_gate` rows.

## Telling "refused: recalibrate" from real failures

Calibration refusals surface as **HTTP 409 Conflict** with a stable `code`,
human `message`, and `details` (`ApiError::Conflict → 409` in
`crates/http-api/src/error.rs:117-121`; refusal mapping in
`crates/http-api/src/routes/ai/generation.rs:300-315`). Codes are defined in
`review_calibration.rs:89-100`.

| What you see | Meaning | Operator action |
| --- | --- | --- |
| `calibration_incompatible`, message "…predates stage-scoped identities (scheme 1)…" or "…predates structured semantic contracts (scheme 2)…" (`review_calibration.rs:111-118`) | Expected post-deploy signal: pre-migration bundle under an old recipe. Also returned when the snapshot is missing/unreadable or does not reproduce the stored hash (`review_calibration.rs:328-339`) | Recalibrate. Not a bug. |
| `calibration_stale` with `details.changes` naming consequential behavior (`generation.rs:301-308`, `review_calibration.rs:336-343`) | Bundle is for this stage but the compiled review changed | Recalibrate that definition; the `changes` list says what moved. |
| `calibration_stage_mismatch` (`review_calibration.rs:315-324`) | Bundle is for another stage / non-screening review | Check stage scoping, not system health. |
| `calibration_missing` / `calibration_failed` | Genuinely absent or not-passing evidence | Record/pass a calibration. Note these two **clear** the second-review gate (`second_review.rs:219-221`), while stale/mismatch/incompatible **set** it (`215-218`). |

Real failures look different:

- 5xx (`INTERNAL_ERROR`, `error.rs:69-88`) and 503 `CONFIGURATION_ERROR`
  for a missing AI provider (`error.rs:129-133`) are infrastructure/config,
  never calibration.
- Worker logs: a stage-wide pause logs "automatic second review is paused
  for this stage" at info level (`second_review.rs:225`); record-specific
  refusals log "second review could not be scheduled" at warn and keep their
  attempt queued (`second_review.rs:228-233`).
- Unrelated 409s such as `review_subject_changed`
  (`generation.rs:256-264`) are not calibration signals.
