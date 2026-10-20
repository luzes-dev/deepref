# Handoff 03c: actor continuity

## Principal to actor

New human HTTP writes persist `actor_kind = 'user'` and the authenticated principal's `UserId` as its canonical lowercase UUID string. The implementation keeps the existing actor kind because the current schema already represents human actors this way; it adds no new actor kind. The request extractor does not read `x-actor-id` or `x-actor-kind`. N-1 clients may continue sending those headers during rollout, and they have no authority. The web fetch wrapper strips both headers, and the reviewer-ID input and helper were removed.

With auth disabled in `APP_ENV=development`, the session gate supplies one fixed principal, `00000000-0000-0000-0000-000000000003`. Other auth-off environments are not granted a fallback user principal. System, automation and AI actors continue to be created by server-side workflows.

## Scientific assignments

`scientific_assignments` stores only the required project/user/role/assigner/timestamp/revocation fields. Its closed roles are `primary_screener`, `secondary_screener`, `consensus_reviewer`, `extractor`, `appraisal_reviewer` and `fresh_ai_auditor`. GET is protected by `project:view`; POST and DELETE resolve the project's workspace and require `workspace:manage`. The existing AI-first fresh-auditor check consumes active `fresh_ai_auditor` assignments. No second-screener assignment flow existed to wire, so the other roles are available without redesigning screening UX.

## Identity continuity

`legacy_actor_mappings` trusts an operator-confirmed `(kind, label) -> UserId`; `legacy_actor_non_identities` records that a specific user is not a label. Mapping keys are byte-exact: no case folding, trimming, or UUID parsing is applied. Both tables accept only `user` labels, and the only application writer is the `deepref-server legacy-actor map|deny|list` operator command. Each mutating command writes its corresponding `admin_audit_events` row in the same transaction. Repeating the same decision is idempotent. A conflicting mapping, or mapping a label already denied for that user, is refused.

Canonical identity comes from trusted provenance. Migration 0107 adds nullable `actor_user_id` foreign keys to `screening_events`, `ai_opinion_exposures` audience, `ai_screening_audit_labels`, project authority approval, and cohort approval/finalization columns. New authenticated writes store the authenticated `UserId` there alongside the existing text label. N-1 writes and historical rows keep NULL; no label is backfilled or inferred from its shape. A history actor is distinct from another only when both resolve consistently, through `actor_user_id` or an exact label mapping, to different `UserId` values. UUID-shaped text stays unresolved until the operator maps it.

`legacy_actor_non_identities` still records that one label is not one current user, but it does not resolve that label to another person. The `users` table is not the universe of possible identities, so negative decisions cannot prove two unresolved labels distinct. Freshness, queue visibility, write-time checks, evaluation, and finalization all use this rule. The old two-argument v2 helpers remain fail-closed. The old text signatures are replaced so N-1 binaries keep calling known functions; the old freshness signature still checks labels across the whole cohort because it has no report argument.

Audit labels are scoped by report for current N reviewers, while the N-1 row count is cohort/report based. A database `BEFORE INSERT` and actor-column `BEFORE UPDATE` trigger on `ai_screening_audit_labels` rejects a row unless it resolves distinctly from every existing label in that report. This makes the legacy row count equal the number of distinct reviewers during rollout. A cohort status transition trigger repeats the distinctness check before `passed` or `finalized`, protecting old evaluators from historical invalid rows and mappings that changed after evaluation. Neither trigger rewrites old evidence.

Every identity check and mapping change shares advisory lock key `73003203` (the one-key `bigint` advisory lock). The N application takes the shared lock first, then the project row lock, then the cohort row lock and report-level locks. The audit-label and terminal-status triggers take the same shared lock. N-1 code cannot take the lock before its existing project/cohort locks, so its database triggers acquire it at the write or terminal transition boundary; mapping commands never take project or cohort locks. The `legacy-actor map` and `legacy-actor deny` commands take the exclusive global lock first, then the per-label advisory lock, then write the decision and administrative audit event in one transaction. This serializes validation with mapping changes without a lock cycle.

The 0107 migration also installs stale-provenance triggers for each actor text/provenance pair in `screening_events`, `ai_opinion_exposures`, `ai_screening_audit_labels`, `project_ai_screening_authority`, and `ai_screening_cohorts`. If an N-1 update changes the text label while leaving its provenance unchanged, the trigger clears the old `actor_user_id`. A mapping that makes finalized evidence fail the two-reviewer rule returns the affected cohort IDs from the command and stores them in its audit metadata for operator review. The mapping command never changes the historical evidence.

Migrations `0103` through `0107` add assignments and decision tables, expand then validate the audit constraint for the new event types, add continuity helpers, and add nullable actor provenance and transition guards. They do not rewrite `screening_events`, `ai_opinion_exposures`, `ai_screening_audit_labels` or cohort approval history.

## Baseline `crates/app` inventory

The inventory below is the committed pre-change baseline from `git grep -n 'actor_id\|audience_actor_id\|approved_by' HEAD -- crates/app`: 333 matching source lines. Each entry carries the original path, line number and complete matching line. `storage` includes schema constraints and persistence writes; `display` includes reads, exports, test fixtures/assertions and presentation. The nine identity comparisons include the 0054 textual audit-label uniqueness rule, the 0054 queue comparison, the 0055 freshness and queue comparisons, and the application exposure and frozen-evidence comparisons. They are linked to behavior tests.

| Classification | Lines |
| --- | ---: |
| identity comparison | 9 |
| storage | 152 |
| display | 172 |
| **Total** | **333** |

### Counts by file

| File | Identity comparison | Storage | Display | Total |
| --- | ---: | ---: | ---: | ---: |
| `crates/app/migrations/0006_evidence_workspace.sql` | 0 | 2 | 0 | 2 |
| `crates/app/migrations/0010_deduplication.sql` | 0 | 1 | 0 | 1 |
| `crates/app/migrations/0012_title_abstract_screening.sql` | 0 | 2 | 0 | 2 |
| `crates/app/migrations/0013_documents_full_text.sql` | 0 | 2 | 0 | 2 |
| `crates/app/migrations/0014_studies_appraisals.sql` | 0 | 5 | 0 | 5 |
| `crates/app/migrations/0016_ai_foundation.sql` | 0 | 4 | 0 | 4 |
| `crates/app/migrations/0018_ai_appraisal_extraction.sql` | 0 | 3 | 0 | 3 |
| `crates/app/migrations/0019_automation_agent.sql` | 0 | 19 | 0 | 19 |
| `crates/app/migrations/0025_manual_extraction_values.sql` | 0 | 4 | 0 | 4 |
| `crates/app/migrations/0028_workflow_engine.sql` | 0 | 1 | 0 | 1 |
| `crates/app/migrations/0029_ai_autonomy_and_activity.sql` | 0 | 2 | 0 | 2 |
| `crates/app/migrations/0049_ai_opinion_exposures.sql` | 0 | 5 | 0 | 5 |
| `crates/app/migrations/0054_ai_first_cohorts.sql` | 2 | 3 | 0 | 5 |
| `crates/app/migrations/0055_ai_first_blind_controls.sql` | 4 | 0 | 0 | 4 |
| `crates/app/migrations/0057_assistant_agent_runs.sql` | 0 | 1 | 0 | 1 |
| `crates/app/migrations/0073_durable_runs.sql` | 0 | 3 | 0 | 3 |
| `crates/app/src/activity.rs` | 0 | 2 | 2 | 4 |
| `crates/app/src/ai.rs` | 0 | 1 | 2 | 3 |
| `crates/app/src/ai/acceptance.rs` | 0 | 1 | 0 | 1 |
| `crates/app/src/ai/persistence.rs` | 0 | 4 | 1 | 5 |
| `crates/app/src/ai/proposals.rs` | 0 | 2 | 2 | 4 |
| `crates/app/src/ai_autonomy_apply.rs` | 0 | 1 | 1 | 2 |
| `crates/app/src/ai_exposure.rs` | 1 | 0 | 4 | 5 |
| `crates/app/src/ai_first/audit.rs` | 2 | 2 | 4 | 8 |
| `crates/app/src/ai_first/cohorts.rs` | 0 | 4 | 0 | 4 |
| `crates/app/src/ai_reviewer.rs` | 0 | 0 | 2 | 2 |
| `crates/app/src/appraisal.rs` | 0 | 2 | 4 | 6 |
| `crates/app/src/assistant_plans.rs` | 0 | 2 | 0 | 2 |
| `crates/app/src/assistant_runs.rs` | 0 | 6 | 4 | 10 |
| `crates/app/src/audit_export.rs` | 0 | 1 | 34 | 35 |
| `crates/app/src/automations.rs` | 0 | 1 | 9 | 10 |
| `crates/app/src/autonomy.rs` | 0 | 0 | 1 | 1 |
| `crates/app/src/deduplication.rs` | 0 | 21 | 19 | 40 |
| `crates/app/src/documents.rs` | 0 | 2 | 4 | 6 |
| `crates/app/src/extraction.rs` | 0 | 6 | 12 | 18 |
| `crates/app/src/protocol.rs` | 0 | 1 | 0 | 1 |
| `crates/app/src/screening.rs` | 0 | 3 | 2 | 5 |
| `crates/app/src/study.rs` | 0 | 4 | 11 | 15 |
| `crates/app/src/workflows.rs` | 0 | 9 | 4 | 13 |
| `crates/app/tests/ai.rs` | 0 | 0 | 5 | 5 |
| `crates/app/tests/ai_first_blinding.rs` | 0 | 0 | 2 | 2 |
| `crates/app/tests/assistant_runs.rs` | 0 | 0 | 1 | 1 |
| `crates/app/tests/automation_events.rs` | 0 | 0 | 2 | 2 |
| `crates/app/tests/deduplication.rs` | 0 | 0 | 23 | 23 |
| `crates/app/tests/document_structure.rs` | 0 | 0 | 1 | 1 |
| `crates/app/tests/documents.rs` | 0 | 0 | 8 | 8 |
| `crates/app/tests/durable_run_migration.rs` | 0 | 1 | 0 | 1 |
| `crates/app/tests/extraction.rs` | 0 | 0 | 2 | 2 |
| `crates/app/tests/extraction_fields.rs` | 0 | 0 | 2 | 2 |
| `crates/app/tests/project_deletion.rs` | 0 | 12 | 0 | 12 |
| `crates/app/tests/protocol.rs` | 0 | 3 | 0 | 3 |
| `crates/app/tests/review_runs.rs` | 0 | 0 | 2 | 2 |
| `crates/app/tests/screening.rs` | 0 | 4 | 0 | 4 |
| `crates/app/tests/study_ungrouped.rs` | 0 | 0 | 1 | 1 |
| `crates/app/tests/workflow_review_counts.rs` | 0 | 0 | 1 | 1 |

### Every matching source line

The `test` column is `—` for storage/display entries and names the behavior test for every identity comparison.

```text
storage | crates/app/migrations/0006_evidence_workspace.sql:116 | test=— |   actor_id text NOT NULL,
storage | crates/app/migrations/0006_evidence_workspace.sql:150 | test=— |   actor_id text NOT NULL,
storage | crates/app/migrations/0010_deduplication.sql:123 | test=— |   actor_id text NOT NULL CHECK (btrim(actor_id) <> ''),
storage | crates/app/migrations/0012_title_abstract_screening.sql:31 | test=— |   DROP CONSTRAINT IF EXISTS screening_events_actor_id_check;
storage | crates/app/migrations/0012_title_abstract_screening.sql:187 | test=— |   ADD CONSTRAINT screening_events_actor_id_check CHECK (length(btrim(actor_id)) > 0);
storage | crates/app/migrations/0013_documents_full_text.sql:13 | test=— |   ADD COLUMN IF NOT EXISTS actor_id text NOT NULL DEFAULT 'legacy-migration',
storage | crates/app/migrations/0013_documents_full_text.sql:82 | test=— |   ADD CONSTRAINT documents_actor_id_check CHECK (length(btrim(actor_id)) > 0),
storage | crates/app/migrations/0014_studies_appraisals.sql:8 | test=— |   ADD COLUMN updated_by_actor_id text NOT NULL DEFAULT 'legacy-migration',
storage | crates/app/migrations/0014_studies_appraisals.sql:44 | test=— |   ADD CONSTRAINT studies_updated_actor_id_check CHECK (length(btrim(updated_by_actor_id)) > 0),
storage | crates/app/migrations/0014_studies_appraisals.sql:94 | test=— |   actor_id text NOT NULL CHECK (length(btrim(actor_id)) > 0),
storage | crates/app/migrations/0014_studies_appraisals.sql:124 | test=— |   actor_id text NOT NULL CHECK (length(btrim(actor_id)) > 0),
storage | crates/app/migrations/0014_studies_appraisals.sql:164 | test=— |   actor_id text NOT NULL CHECK (length(btrim(actor_id)) > 0),
storage | crates/app/migrations/0016_ai_foundation.sql:137 | test=— |   ADD COLUMN IF NOT EXISTS resolved_by_actor_id text,
storage | crates/app/migrations/0016_ai_foundation.sql:147 | test=— |     resolved_by_actor_id = CASE WHEN status = 'pending' THEN NULL ELSE COALESCE(NULLIF(resolved_by_actor_id, ''), NULLIF(decided_by, ''), 'legacy-migration') END;
storage | crates/app/migrations/0016_ai_foundation.sql:161 | test=— |     (status = 'pending' AND resolved_at IS NULL AND resolved_by_actor_kind IS NULL AND resolved_by_actor_id IS NULL)
storage | crates/app/migrations/0016_ai_foundation.sql:164 | test=— |         AND resolved_by_actor_id IS NOT NULL AND length(btrim(resolved_by_actor_id)) > 0)
storage | crates/app/migrations/0018_ai_appraisal_extraction.sql:54 | test=— |   approved_by_actor_kind text NOT NULL CHECK (approved_by_actor_kind IN ('user', 'automation', 'system')),
storage | crates/app/migrations/0018_ai_appraisal_extraction.sql:55 | test=— |   approved_by_actor_id text NOT NULL CHECK (length(btrim(approved_by_actor_id)) > 0),
storage | crates/app/migrations/0018_ai_appraisal_extraction.sql:97 | test=— |   actor_id text NOT NULL CHECK (length(btrim(actor_id)) > 0),
storage | crates/app/migrations/0019_automation_agent.sql:72 | test=— |   actor_id text NOT NULL CHECK (length(btrim(actor_id)) BETWEEN 1 AND 200),
storage | crates/app/migrations/0019_automation_agent.sql:113 | test=— |   actor_id text NOT NULL CHECK (length(btrim(actor_id)) BETWEEN 1 AND 200),
storage | crates/app/migrations/0019_automation_agent.sql:252 | test=— |   target_actor_id text
storage | crates/app/migrations/0019_automation_agent.sql:263 | test=— |   actor_id text,
storage | crates/app/migrations/0019_automation_agent.sql:289 | test=— |       OR target_actor_id IS NULL OR length(btrim(target_actor_id)) NOT BETWEEN 1 AND 200 THEN
storage | crates/app/migrations/0019_automation_agent.sql:302 | test=— |      actor_kind, actor_id)
storage | crates/app/migrations/0019_automation_agent.sql:305 | test=— |      target_recipe_version, target_status, target_actor_kind, target_actor_id)
storage | crates/app/migrations/0019_automation_agent.sql:312 | test=— |     actor_id = EXCLUDED.actor_id,
storage | crates/app/migrations/0019_automation_agent.sql:328 | test=— |          d.status, d.actor_kind, d.actor_id, d.created_at, d.updated_at
storage | crates/app/migrations/0019_automation_agent.sql:344 | test=— |   actor_id text,
storage | crates/app/migrations/0019_automation_agent.sql:353 | test=— |          d.status, d.actor_kind, d.actor_id, d.created_at, d.updated_at,
storage | crates/app/migrations/0019_automation_agent.sql:379 | test=— |   target_actor_id text
storage | crates/app/migrations/0019_automation_agent.sql:422 | test=— |       OR target_actor_id IS NULL OR length(btrim(target_actor_id)) NOT BETWEEN 1 AND 200 THEN
storage | crates/app/migrations/0019_automation_agent.sql:481 | test=— |      trigger_kind, trigger_reference, idempotency_key, actor_kind, actor_id)
storage | crates/app/migrations/0019_automation_agent.sql:485 | test=— |      target_trigger_reference, target_idempotency_key, target_actor_kind, target_actor_id)
storage | crates/app/migrations/0019_automation_agent.sql:527 | test=— |   target_actor_id text
storage | crates/app/migrations/0019_automation_agent.sql:544 | test=— |     target_actor_id
storage | crates/app/migrations/0019_automation_agent.sql:561 | test=— |   r.actor_id,
storage | crates/app/migrations/0019_automation_agent.sql:710 | test=— |       OR OLD.actor_id IS DISTINCT FROM NEW.actor_id THEN
storage | crates/app/migrations/0025_manual_extraction_values.sql:18 | test=— |   ADD COLUMN superseded_by_actor_id text
storage | crates/app/migrations/0025_manual_extraction_values.sql:19 | test=— |     CHECK (length(btrim(superseded_by_actor_id)) > 0),
storage | crates/app/migrations/0025_manual_extraction_values.sql:28 | test=— |     (superseded_at IS NULL AND superseded_by_actor_kind IS NULL AND superseded_by_actor_id IS NULL)
storage | crates/app/migrations/0025_manual_extraction_values.sql:30 | test=— |        AND superseded_by_actor_id IS NOT NULL)
storage | crates/app/migrations/0028_workflow_engine.sql:96 | test=— |   actor_id text NOT NULL CHECK (length(btrim(actor_id)) BETWEEN 1 AND 200),
storage | crates/app/migrations/0029_ai_autonomy_and_activity.sql:37 | test=— |   actor_id text NOT NULL,
storage | crates/app/migrations/0029_ai_autonomy_and_activity.sql:103 | test=— |   ADD COLUMN verified_by_actor_id text,
storage | crates/app/migrations/0049_ai_opinion_exposures.sql:44 | test=— |   audience_actor_id text CHECK (audience_actor_id IS NULL OR length(btrim(audience_actor_id)) > 0),
storage | crates/app/migrations/0049_ai_opinion_exposures.sql:55 | test=— |     (audience_actor_kind IS NULL AND audience_actor_id IS NULL)
storage | crates/app/migrations/0049_ai_opinion_exposures.sql:56 | test=— |     OR (audience_actor_kind IS NOT NULL AND audience_actor_id IS NOT NULL)
storage | crates/app/migrations/0049_ai_opinion_exposures.sql:68 | test=— |       COALESCE(audience_actor_kind, ''), COALESCE(audience_actor_id, ''))
storage | crates/app/migrations/0049_ai_opinion_exposures.sql:72 | test=— |       COALESCE(audience_actor_kind, ''), COALESCE(audience_actor_id, ''))
storage | crates/app/migrations/0054_ai_first_cohorts.sql:7 | test=— |  approved_by text NOT NULL,
storage | crates/app/migrations/0054_ai_first_cohorts.sql:26 | test=— |  approved_by text NOT NULL, final_approval_id uuid, finalized_by text,
storage | crates/app/migrations/0054_ai_first_cohorts.sql:70 | test=— |  cohort_id uuid NOT NULL, report_id uuid NOT NULL, actor_id text NOT NULL,
identity comparison | crates/app/migrations/0054_ai_first_cohorts.sql:73 | test=mapped_legacy_audit_label_blocks_direct_duplicate_screening_post |  UNIQUE(cohort_id,report_id,actor_id),
identity comparison | crates/app/migrations/0054_ai_first_cohorts.sql:143 | test=the_0054_queue_comparison_is_enumerated_and_superseded_by_0055; the_n_minus_one_queue_still_matches_legacy_audit_labels |          WHERE l.cohort_id=c.id AND l.report_id=report AND l.actor_id=actor)))))
identity comparison | crates/app/migrations/0055_ai_first_blind_controls.sql:13 | test=n_minus_one_approval_identity_comparison_stays_text_based |  c.approved_by=actor OR EXISTS(SELECT 1 FROM ai_screening_cohort_members m JOIN screening_events e
identity comparison | crates/app/migrations/0055_ai_first_blind_controls.sql:15 | test=n_minus_one_screening_event_identity_comparison_stays_text_based |  WHERE m.cohort_id=c.id AND e.actor_kind='user' AND e.actor_id=actor)
identity comparison | crates/app/migrations/0055_ai_first_blind_controls.sql:19 | test=n_minus_one_exposure_audience_identity_comparison_stays_text_based |  AND (e.audience_actor_id IS NULL OR (e.audience_actor_kind='user' AND e.audience_actor_id=actor)))))
identity comparison | crates/app/migrations/0055_ai_first_blind_controls.sql:27 | test=the_n_minus_one_queue_still_matches_legacy_audit_labels |  AND NOT EXISTS(SELECT 1 FROM ai_screening_audit_labels l WHERE l.cohort_id=c.id AND l.report_id=report AND l.actor_id=actor)
storage | crates/app/migrations/0057_assistant_agent_runs.sql:17 | test=— |   actor_id text NOT NULL,
storage | crates/app/migrations/0073_durable_runs.sql:23 | test=— |   actor_id text NOT NULL,
storage | crates/app/migrations/0073_durable_runs.sql:68 | test=— | SELECT id,project_id,'automation',job_id,actor_kind,actor_id,idempotency_key,status,error,created_at,started_at,finished_at FROM automation_runs;
storage | crates/app/migrations/0073_durable_runs.sql:74 | test=— | SELECT id,project_id,'workflow',NULL,actor_kind,actor_id,idempotency_key,status,error,created_at,started_at,finished_at FROM workflow_runs;
display | crates/app/src/activity.rs:198 | test=— |          (id,project_id,actor_type,actor_label,actor_kind,actor_id,task,action,summary,affected,
storage | crates/app/src/activity.rs:398 | test=— |                  SET superseded_at=now(),superseded_by_actor_kind=$3,superseded_by_actor_id=$4
storage | crates/app/src/activity.rs:428 | test=— |                     actor_id: actor.id().to_owned(),
display | crates/app/src/activity.rs:502 | test=— |          (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
storage | crates/app/src/ai.rs:586 | test=— |     pub resolved_by_actor_id: Option<String>,
display | crates/app/src/ai.rs:953 | test=— |             let row = sqlx::query("SELECT id,project_id,entity_type,entity_id,operation,payload,authority_tier,model_run_id,status,resolved_at,resolved_by_actor_id FROM ai_proposals WHERE model_run_id=$1 ORDER BY created_at DESC,id DESC LIMIT 1")
display | crates/app/src/ai.rs:1057 | test=— |                 model_run_id,status,resolved_at,resolved_by_actor_id
storage | crates/app/src/ai/acceptance.rs:794 | test=— |         resolved_by_actor_id: row.get("resolved_by_actor_id"),
storage | crates/app/src/ai/persistence.rs:40 | test=— |     actor_id: &str,
display | crates/app/src/ai/persistence.rs:43 | test=— |     if !matches!(actor_kind, "user" | "automation" | "system") || actor_id.trim().is_empty() {
storage | crates/app/src/ai/persistence.rs:51 | test=— |     let result = sqlx::query("UPDATE ai_proposals SET status=$2,resolved_at=now(),resolved_by_actor_kind=$3,resolved_by_actor_id=$4,resolution_reason=$5,decided_by=$4,decided_at=now() WHERE id=$1 AND status='pending'")
storage | crates/app/src/ai/persistence.rs:52 | test=— |         .bind(proposal_id).bind(status.as_str()).bind(actor_kind).bind(actor_id).bind(reason).execute(pool).await?;
storage | crates/app/src/ai/persistence.rs:182 | test=— |         resolved_by_actor_id: row.get("resolved_by_actor_id"),
display | crates/app/src/ai/proposals.rs:25 | test=— |             p.target_record_id,p.target_study_id,p.resolved_at,p.resolved_by_actor_kind,p.resolved_by_actor_id,
storage | crates/app/src/ai/proposals.rs:233 | test=— |          SET status=$3,resolved_at=now(),resolved_by_actor_kind=$4,resolved_by_actor_id=$5,
display | crates/app/src/ai/proposals.rs:250 | test=— |          (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
storage | crates/app/src/ai/proposals.rs:352 | test=— |             actor_id: actor.id().to_owned(),
storage | crates/app/src/ai_autonomy_apply.rs:170 | test=— |          SET status=$3,resolved_at=now(),resolved_by_actor_kind=$4,resolved_by_actor_id=$5,
display | crates/app/src/ai_autonomy_apply.rs:187 | test=— |          (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
display | crates/app/src/ai_exposure.rs:93 | test=— |       (project_id, report_id, stage, exposure_source, audience_actor_kind, audience_actor_id,
display | crates/app/src/ai_exposure.rs:298 | test=— |                     AND (o.audience_actor_id IS NULL OR t.actor_id IS NULL
identity comparison | crates/app/src/ai_exposure.rs:299 | test=an_unmapped_legacy_exposure_excludes_until_operator_non_identity |                          OR (o.audience_actor_kind=t.actor_kind AND o.audience_actor_id=t.actor_id))
display | crates/app/src/ai_exposure.rs:309 | test=— |                   h.actor_kind, h.actor_id
display | crates/app/src/ai_exposure.rs:322 | test=— |              SELECT e.created_at, e.actor_kind, e.actor_id, e.event_kind, e.decision,
identity comparison | crates/app/src/ai_first/audit.rs:276 | test=audit_label_precheck_matches_canonical_and_operator_mapped_users |       AND (audience_actor_id IS NULL OR (audience_actor_kind='user' AND audience_actor_id=$3)))",
storage | crates/app/src/ai_first/audit.rs:315 | test=— |         "INSERT INTO ai_screening_audit_labels(project_id,cohort_id,report_id,actor_id,decision)
storage | crates/app/src/ai_first/audit.rs:316 | test=— |       VALUES($1,$2,$3,$4,$5) ON CONFLICT(cohort_id,report_id,actor_id) DO NOTHING",
display | crates/app/src/ai_first/audit.rs:340 | test=— |       min(l.actor_id) AS actor_id FROM ai_screening_cohort_members m LEFT JOIN screening_state s ON s.project_id=m.project_id AND s.report_id=m.report_id JOIN ai_screening_audit_labels l
display | crates/app/src/ai_first/audit.rs:355 | test=— |             actor:Actor::new(ActorKind::User,row.get::<String,_>("actor_id")).map_err(|e|AiFirstError::Invalid(e.to_string()))?,
display | crates/app/src/ai_first/audit.rs:525 | test=— |                 "semantic_bundle_hash":c.get::<String,_>("semantic_bundle_hash"),"approved_by":actor.id()}).to_string()),actor:automation.clone(),
display | crates/app/src/ai_first/audit.rs:537 | test=— |         "actor_identity":"self_asserted"}),
identity comparison | crates/app/src/ai_first/audit.rs:555 | test=frozen_evidence_comparison_respects_mappings_and_non_identity_records |        AND (e.audience_actor_id IS NULL OR (e.audience_actor_kind='user' AND e.audience_actor_id=l.actor_id)))")
storage | crates/app/src/ai_first/cohorts.rs:74 | test=— |     sqlx::query("INSERT INTO review_events (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
storage | crates/app/src/ai_first/cohorts.rs:221 | test=— |     sqlx::query("INSERT INTO project_ai_screening_authority(project_id,ceiling,approved_by) VALUES($1,$2,$3)
storage | crates/app/src/ai_first/cohorts.rs:222 | test=— |        ON CONFLICT(project_id) DO UPDATE SET ceiling=$2,approved_by=$3,suspended_reason=NULL,updated_at=now()")
storage | crates/app/src/ai_first/cohorts.rs:232 | test=— |     sqlx::query("INSERT INTO ai_screening_cohorts(id,project_id,protocol_version_id,semantic_bundle_hash,semantic_identity,policy_version,target_percent,approved_by)
display | crates/app/src/ai_reviewer.rs:119 | test=— |            CASE WHEN e.event_kind='decision' AND e.actor_kind='user' AND NOT ai_first_blinded(d.project_id,d.report_id) THEN e.actor_id END AS human_actor
display | crates/app/src/ai_reviewer.rs:421 | test=— |          (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
storage | crates/app/src/appraisal.rs:31 | test=— |     pub actor_id: String,
display | crates/app/src/appraisal.rs:62 | test=— |                 responses, judgments, actor_kind, actor_id, completed_at, created_at
display | crates/app/src/appraisal.rs:123 | test=— |            actor_kind,actor_id)
display | crates/app/src/appraisal.rs:153 | test=— |           (id,assessment_id,project_id,report_id,event_type,payload,actor_kind,actor_id)
display | crates/app/src/appraisal.rs:188 | test=— |                 responses, judgments, actor_kind, actor_id, completed_at, created_at
storage | crates/app/src/appraisal.rs:231 | test=— |         actor_id: row.get("actor_id"),
storage | crates/app/src/assistant_plans.rs:142 | test=— |     actor_id: &str,
storage | crates/app/src/assistant_plans.rs:159 | test=— |     .bind(actor_id)
storage | crates/app/src/assistant_runs.rs:30 | test=— |     pub actor_id: String,
storage | crates/app/src/assistant_runs.rs:76 | test=— |     pub actor_id: String,
storage | crates/app/src/assistant_runs.rs:105 | test=— |         actor_id: row.get("actor_id"),
storage | crates/app/src/assistant_runs.rs:188 | test=— |     actor_id: &str,
storage | crates/app/src/assistant_runs.rs:216 | test=— |             actor_id: actor_id.to_owned(),
display | crates/app/src/assistant_runs.rs:284 | test=— |            (id, conversation_id, project_id, trigger_message_id, actor_kind, actor_id,
storage | crates/app/src/assistant_runs.rs:295 | test=— |     .bind(&input.actor_id)
display | crates/app/src/assistant_runs.rs:314 | test=— |         "SELECT id, conversation_id, project_id, trigger_message_id, actor_kind, actor_id, \
display | crates/app/src/assistant_runs.rs:330 | test=— |         "SELECT id, conversation_id, project_id, trigger_message_id, actor_kind, actor_id, \
display | crates/app/src/assistant_runs.rs:364 | test=— |         "SELECT id, conversation_id, project_id, trigger_message_id, actor_kind, actor_id, \
storage | crates/app/src/audit_export.rs:14 | test=— |     pub actor_id: Option<String>,
display | crates/app/src/audit_export.rs:36 | test=— |                   actor_kind, actor_id, protocol_version_id, stage, decision,
display | crates/app/src/audit_export.rs:41 | test=— |                     report_id AS aggregate_id, actor_kind, actor_id, protocol_version_id,
display | crates/app/src/audit_export.rs:62 | test=— |                     jsonb_build_object('actor_kind', actor_kind, 'actor_id', actor_id,
display | crates/app/src/audit_export.rs:67 | test=— |                     study_id AS aggregate_id, actor_kind, actor_id, NULL::uuid,
display | crates/app/src/audit_export.rs:77 | test=— |                     jsonb_build_object('actor_kind', actor_kind, 'actor_id', actor_id,
display | crates/app/src/audit_export.rs:82 | test=— |                     assessment_id AS aggregate_id, actor_kind, actor_id, NULL::uuid,
display | crates/app/src/audit_export.rs:86 | test=— |                     jsonb_build_object('actor_kind', actor_kind, 'actor_id', actor_id,
display | crates/app/src/audit_export.rs:91 | test=— |                     record_id AS aggregate_id, actor_kind, actor_id, NULL::uuid,
display | crates/app/src/audit_export.rs:112 | test=— |                       'actor_kind', actor_kind, 'actor_id', actor_id,
display | crates/app/src/audit_export.rs:120 | test=— |                     a.id AS aggregate_id, NULL::text AS actor_kind, NULL::text AS actor_id,
display | crates/app/src/audit_export.rs:195 | test=— |                     p.resolved_by_actor_id AS actor_id,
display | crates/app/src/audit_export.rs:213 | test=— |                       'resolved_by_actor_id', p.resolved_by_actor_id,
display | crates/app/src/audit_export.rs:244 | test=— |                       'resolved_by_actor_id', p.resolved_by_actor_id,
display | crates/app/src/audit_export.rs:256 | test=— |                       'reviewer_actor_id', p.resolved_by_actor_id,
display | crates/app/src/audit_export.rs:266 | test=— |                     d.actor_kind, d.actor_id, NULL::uuid AS protocol_version_id,
display | crates/app/src/audit_export.rs:279 | test=— |                       'actor_id', d.actor_id
display | crates/app/src/audit_export.rs:292 | test=— |                       'actor_id', d.actor_id,
display | crates/app/src/audit_export.rs:299 | test=— |                       'initiator_actor_id', d.actor_id,
display | crates/app/src/audit_export.rs:307 | test=— |                     r.actor_kind, r.actor_id, NULL::uuid AS protocol_version_id,
display | crates/app/src/audit_export.rs:357 | test=— |                       'initiator_actor_id', r.actor_id,
display | crates/app/src/audit_export.rs:381 | test=— |                     NULL::text AS actor_kind, NULL::text AS actor_id,
display | crates/app/src/audit_export.rs:414 | test=— |                     r.actor_kind, r.actor_id, NULL::uuid AS protocol_version_id,
display | crates/app/src/audit_export.rs:453 | test=— |                       'initiator_actor_id', r.actor_id,
display | crates/app/src/audit_export.rs:480 | test=— |                     r.actor_kind, r.actor_id, NULL::uuid AS protocol_version_id,
display | crates/app/src/audit_export.rs:519 | test=— |                     NULL::text AS actor_kind, a.worker_id AS actor_id,
display | crates/app/src/audit_export.rs:568 | test=— |                     NULL::text AS actor_kind, NULL::text AS actor_id,
display | crates/app/src/audit_export.rs:600 | test=— |                     o.audience_actor_kind, o.audience_actor_id, NULL::uuid AS protocol_version_id,
display | crates/app/src/audit_export.rs:611 | test=— |                       'audience_actor_id', o.audience_actor_id,
display | crates/app/src/audit_export.rs:620 | test=— |              SELECT c.id,c.created_at,'ai_first_cohort','ai_first_cohort',c.id,'user',c.approved_by,
display | crates/app/src/audit_export.rs:623 | test=— |                 jsonb_build_object('reference_claim','conditional_reference_retention_only','actor_identity','self_asserted')
display | crates/app/src/audit_export.rs:632 | test=— |              SELECT l.id,l.created_at,'ai_first_reference_label','report',l.report_id,'user',l.actor_id,
display | crates/app/src/audit_export.rs:647 | test=— |                     c.reviewer_metadata->>'reviewer_id' AS actor_id,
display | crates/app/src/audit_export.rs:685 | test=— |                     p.resolved_by_actor_id AS actor_id,
display | crates/app/src/audit_export.rs:706 | test=— |                       'reviewer_actor_id', p.resolved_by_actor_id,
display | crates/app/src/automations.rs:63 | test=— |                 status, actor_kind, actor_id, created_at, updated_at
display | crates/app/src/automations.rs:88 | test=— |                 status, actor_kind, actor_id, created_at, updated_at, steps
display | crates/app/src/automations.rs:101 | test=— |  r.actor_kind,r.actor_id,r.status,r.created_at,r.started_at,r.finished_at,r.error AS run_error,
storage | crates/app/src/automations.rs:176 | test=— |     sqlx::query("INSERT INTO durable_runs(id,project_id,kind,status,job_id,actor_kind,actor_id,idempotency_key)
display | crates/app/src/automations.rs:274 | test=— |     let (source_actor_kind, source_actor_id) = event.actor();
display | crates/app/src/automations.rs:275 | test=— |     let (actor_kind, actor_id) = if source_actor_id.len() <= 200 {
display | crates/app/src/automations.rs:276 | test=— |         (source_actor_kind, source_actor_id)
display | crates/app/src/automations.rs:303 | test=— |             (actor_kind, actor_id),
display | crates/app/src/automations.rs:440 | test=— |             &row.get::<String, _>("actor_id"),
display | crates/app/src/automations.rs:524 | test=— |             &row.get::<String, _>("actor_id"),
display | crates/app/src/autonomy.rs:101 | test=— |          (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
storage | crates/app/src/deduplication.rs:48 | test=— |     pub actor_id: String,
storage | crates/app/src/deduplication.rs:109 | test=— |     pub actor_id: String,
storage | crates/app/src/deduplication.rs:128 | test=— |     actor_id: &'a str,
storage | crates/app/src/deduplication.rs:140 | test=— |     actor_id: &'a str,
display | crates/app/src/deduplication.rs:200 | test=— |             &request.actor_id,
storage | crates/app/src/deduplication.rs:276 | test=— |     actor_id: &str,
display | crates/app/src/deduplication.rs:291 | test=— |         actor_id,
display | crates/app/src/deduplication.rs:307 | test=— |             actor_id,
display | crates/app/src/deduplication.rs:351 | test=— |         actor_id,
storage | crates/app/src/deduplication.rs:406 | test=— |     actor_id: &str,
display | crates/app/src/deduplication.rs:451 | test=— |                 actor_id,
display | crates/app/src/deduplication.rs:462 | test=— |             actor_id,
storage | crates/app/src/deduplication.rs:511 | test=— |     actor_id: &str,
display | crates/app/src/deduplication.rs:519 | test=— |     let Ok(actor) = Actor::new(kind, actor_id) else {
storage | crates/app/src/deduplication.rs:582 | test=— |                 actor_id: request.actor_id.clone(),
display | crates/app/src/deduplication.rs:593 | test=— |         let Ok(actor) = Actor::new(kind, request.actor_id.clone()) else {
storage | crates/app/src/deduplication.rs:624 | test=— |     actor_id: &str,
display | crates/app/src/deduplication.rs:637 | test=— |             actor_id,
storage | crates/app/src/deduplication.rs:1000 | test=— |             actor_id: link.actor_id,
display | crates/app/src/deduplication.rs:1061 | test=— |     validate_actor(&request.actor_kind, &request.actor_id)?;
storage | crates/app/src/deduplication.rs:1110 | test=— |                 actor_id: request.actor_id.clone(),
display | crates/app/src/deduplication.rs:1143 | test=— |                 &request.actor_id,
storage | crates/app/src/deduplication.rs:1159 | test=— |     .bind(&request.actor_id)
display | crates/app/src/deduplication.rs:1165 | test=— |          (id,project_id,record_id,prior_report_id,resolved_report_id,action,reason,actor_kind,actor_id,proposal_id,reverted_event_id)
storage | crates/app/src/deduplication.rs:1174 | test=— |     .bind(&request.actor_id)
display | crates/app/src/deduplication.rs:1187 | test=— |             &request.actor_id,
storage | crates/app/src/deduplication.rs:1210 | test=— |     actor_id: &str,
display | crates/app/src/deduplication.rs:1231 | test=— |             actor_id,
storage | crates/app/src/deduplication.rs:1247 | test=— |     actor_id: &str,
storage | crates/app/src/deduplication.rs:1269 | test=— |     .bind(actor_id)
display | crates/app/src/deduplication.rs:1338 | test=— |     validate_actor(&request.actor_kind, &request.actor_id)?;
display | crates/app/src/deduplication.rs:1360 | test=— |     validate_actor(&request.actor_kind, &request.actor_id)?;
storage | crates/app/src/deduplication.rs:1436 | test=— |                     actor_id: &request.actor_id,
storage | crates/app/src/deduplication.rs:1471 | test=— |                     actor_id: &request.actor_id,
storage | crates/app/src/deduplication.rs:1528 | test=— |                     actor_id: &request.actor_id,
display | crates/app/src/deduplication.rs:1547 | test=— |             &request.actor_id,
storage | crates/app/src/deduplication.rs:1560 | test=— |         .bind(&request.actor_id)
display | crates/app/src/deduplication.rs:1624 | test=— |          (id,project_id,record_id,prior_report_id,resolved_report_id,action,reason,actor_kind,actor_id,proposal_id,reverted_event_id)
storage | crates/app/src/deduplication.rs:1635 | test=— |     .bind(event.actor_id)
display | crates/app/src/deduplication.rs:1646 | test=— |             "actor_kind must be user, automation, or system and actor_id must not be empty"
storage | crates/app/src/documents.rs:106 | test=— |     pub actor_id: &'a str,
display | crates/app/src/documents.rs:339 | test=— |             source,status,external_url,actor_kind,actor_id,content_available_at,updated_at)
storage | crates/app/src/documents.rs:357 | test=— |     .bind(document.actor_id)
display | crates/app/src/documents.rs:363 | test=— |         let actor = Actor::new(actor_kind, document.actor_id)?;
display | crates/app/src/documents.rs:579 | test=— |          RETURNING actor_kind, actor_id",
display | crates/app/src/documents.rs:591 | test=— |         let actor = Actor::new(actor_kind, updated.actor_id)?;
storage | crates/app/src/extraction.rs:33 | test=— |     pub approved_by_actor_kind: String,
storage | crates/app/src/extraction.rs:34 | test=— |     pub approved_by_actor_id: String,
display | crates/app/src/extraction.rs:278 | test=— |                 source_parser_version,source_content_hash,approved_by_actor_kind,
display | crates/app/src/extraction.rs:279 | test=— |                 approved_by_actor_id,approved_at,needs_verification,verified_at,
display | crates/app/src/extraction.rs:280 | test=— |                 verified_by_actor_id
storage | crates/app/src/extraction.rs:386 | test=— |          SET superseded_at=now(), superseded_by_actor_kind=$5, superseded_by_actor_id=$6
display | crates/app/src/extraction.rs:407 | test=— |           source_content_hash,approved_by_actor_kind,approved_by_actor_id)
display | crates/app/src/extraction.rs:412 | test=— |                    source_parser_version,source_content_hash,approved_by_actor_kind,
display | crates/app/src/extraction.rs:413 | test=— |                    approved_by_actor_id,approved_at,needs_verification,verified_at,
display | crates/app/src/extraction.rs:414 | test=— |                    verified_by_actor_id",
storage | crates/app/src/extraction.rs:471 | test=— |          SET superseded_at=now(), superseded_by_actor_kind=$4, superseded_by_actor_id=$5
display | crates/app/src/extraction.rs:716 | test=— |               source_content_hash,approved_by_actor_kind,approved_by_actor_id,needs_verification)
display | crates/app/src/extraction.rs:754 | test=— |          (id,project_id,study_id,proposal_id,event_type,payload,actor_kind,actor_id)
display | crates/app/src/extraction.rs:780 | test=— |              verified_by_actor_kind=$4,verified_by_actor_id=$5
display | crates/app/src/extraction.rs:796 | test=— |          (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
storage | crates/app/src/extraction.rs:873 | test=— |         approved_by_actor_kind: row.get("approved_by_actor_kind"),
storage | crates/app/src/extraction.rs:874 | test=— |         approved_by_actor_id: row.get("approved_by_actor_id"),
display | crates/app/src/extraction.rs:878 | test=— |         verified_by: row.get("verified_by_actor_id"),
storage | crates/app/src/protocol.rs:403 | test=— |         "INSERT INTO review_events (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id) VALUES ($1,$2,'protocol_published','protocol_version',$3,$4,$5,$6)",
storage | crates/app/src/screening.rs:112 | test=— |     pub actor_id: String,
display | crates/app/src/screening.rs:513 | test=— |         SELECT id,event_kind,stage,decision,notes,protocol_version_id,actor_kind,actor_id,
display | crates/app/src/screening.rs:562 | test=— |           protocol_version_id,actor_kind,actor_id,supersedes_event_id,undoes_event_id,
storage | crates/app/src/screening.rs:640 | test=— |         "INSERT INTO review_events (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id) VALUES ($1,$2,$3,'report',$4,$5,$6,$7)",
storage | crates/app/src/screening.rs:1106 | test=— |         actor_id: row.get("actor_id"),
storage | crates/app/src/study.rs:37 | test=— |     pub updated_by_actor_id: String,
storage | crates/app/src/study.rs:75 | test=— |     pub actor_id: String,
display | crates/app/src/study.rs:122 | test=— |                 s.updated_by_actor_kind, s.updated_by_actor_id
display | crates/app/src/study.rs:258 | test=— |                 actor_kind, actor_id, created_at
display | crates/app/src/study.rs:309 | test=— |            study_revision, updated_by_actor_kind, updated_by_actor_id)
display | crates/app/src/study.rs:380 | test=— |              updated_by_actor_kind=$3, updated_by_actor_id=$4, updated_at=now()
display | crates/app/src/study.rs:464 | test=— |              study_revision=$3, updated_by_actor_kind=$4, updated_by_actor_id=$5,
display | crates/app/src/study.rs:611 | test=— |                  updated_by_actor_id=$3, updated_at=now()
display | crates/app/src/study.rs:634 | test=— |              updated_by_actor_id=$3, updated_at=now()
display | crates/app/src/study.rs:690 | test=— |            study_revision, updated_by_actor_kind, updated_by_actor_id)
display | crates/app/src/study.rs:784 | test=— |              updated_by_actor_id=$3, updated_at=now()
display | crates/app/src/study.rs:908 | test=— |                 updated_by_actor_kind, updated_by_actor_id
storage | crates/app/src/study.rs:945 | test=— |         updated_by_actor_id: row.get("updated_by_actor_id"),
display | crates/app/src/study.rs:1014 | test=— |            before_revision,result_revision,before_snapshot,result_snapshot,payload,actor_kind,actor_id)
storage | crates/app/src/study.rs:1058 | test=— |         actor_id: row.get("actor_id"),
storage | crates/app/src/workflows.rs:179 | test=— |     pub actor_id: String,
storage | crates/app/src/workflows.rs:941 | test=— |     pub actor_id: String,
display | crates/app/src/workflows.rs:1007 | test=— |          (id,project_id,kind,status,actor_kind,actor_id,idempotency_key,started_at)
storage | crates/app/src/workflows.rs:1013 | test=— |     .bind(&request.actor_id)
display | crates/app/src/workflows.rs:1354 | test=— |         v.version, d.trigger_kind, r.status, d.test_mode, r.actor_kind, r.actor_id,
storage | crates/app/src/workflows.rs:1377 | test=— |         actor_id: row.get("actor_id"),
storage | crates/app/src/workflows.rs:1526 | test=— |     pub actor_id: String,
display | crates/app/src/workflows.rs:1553 | test=— |                 d.wait_state, r.actor_kind, r.actor_id, w.graph, w.test_mode,
storage | crates/app/src/workflows.rs:1598 | test=— |         actor_id: row.get("actor_id"),
storage | crates/app/src/workflows.rs:1963 | test=— |                     actor_id: "workflow-schedule".to_owned(),
storage | crates/app/src/workflows.rs:2050 | test=— |             actor_id: "workflow-alert".to_owned(),
display | crates/app/src/workflows.rs:2097 | test=— |     let (actor_kind, actor_id) = event.actor();
storage | crates/app/src/workflows.rs:2114 | test=— |             actor_id: actor_id.to_owned(),
display | crates/app/tests/ai.rs:51 | test=— |             actor_kind,actor_id,active_parser_version,parser_version)
display | crates/app/tests/ai.rs:169 | test=— |          (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
display | crates/app/tests/ai.rs:314 | test=— |         resolved_by_actor_id: None,
display | crates/app/tests/ai.rs:405 | test=— |         resolved_by_actor_id: None,
display | crates/app/tests/ai.rs:629 | test=— |           actor_kind,actor_id,active_parser_version,parser_version)
display | crates/app/tests/ai_first_blinding.rs:110 | test=— |                (id,project_id,protocol_version_id,semantic_bundle_hash,semantic_identity,policy_version,status,target_percent,approved_by)
display | crates/app/tests/ai_first_blinding.rs:279 | test=— |             actor_id: "blinding-test".to_owned(),
display | crates/app/tests/assistant_runs.rs:49 | test=— |         actor_id: "tester".to_owned(),
display | crates/app/tests/automation_events.rs:138 | test=— |             "SELECT d.trigger_kind,d.trigger_reference,d.idempotency_key,r.actor_kind,r.actor_id
display | crates/app/tests/automation_events.rs:160 | test=— |                 .all(|row| row.get::<String, _>("actor_id") == "automation-domain-event")
display | crates/app/tests/deduplication.rs:80 | test=— |         actor_id: "dedupe-test".to_owned(),
display | crates/app/tests/deduplication.rs:292 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:377 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:459 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:703 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:727 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:825 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:854 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:908 | test=— |          (id,project_id,record_id,prior_report_id,action,reason,actor_kind,actor_id)
display | crates/app/tests/deduplication.rs:965 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1149 | test=— |                 actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1165 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1200 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1232 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1271 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1286 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1301 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1359 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1408 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1530 | test=— |         actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1600 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1631 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/deduplication.rs:1765 | test=— |             actor_id: "tester".to_owned(),
display | crates/app/tests/document_structure.rs:53 | test=— |             actor_id: "structure-tester",
display | crates/app/tests/documents.rs:85 | test=— |             actor_id: "document-tester",
display | crates/app/tests/documents.rs:174 | test=— |             actor_id: "document-tester",
display | crates/app/tests/documents.rs:324 | test=— |             actor_id: "parser-test",
display | crates/app/tests/documents.rs:435 | test=— |                 actor_id: "dedupe-test",
display | crates/app/tests/documents.rs:459 | test=— |             actor_id: "dedupe-test",
display | crates/app/tests/documents.rs:499 | test=— |             actor_id: "queue-test",
display | crates/app/tests/documents.rs:545 | test=— |             actor_id: "retrieval-race-test",
display | crates/app/tests/documents.rs:627 | test=— |             actor_id: "requeue-test",
storage | crates/app/tests/durable_run_migration.rs:85 | test=— |     sqlx::query("INSERT INTO workflow_runs(id,project_id,workflow_id,graph,trigger_kind,idempotency_key,actor_kind,actor_id,status,started_at)
display | crates/app/tests/extraction.rs:45 | test=— |          (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
display | crates/app/tests/extraction.rs:66 | test=— |           actor_kind,actor_id,active_parser_version,parser_version)
display | crates/app/tests/extraction_fields.rs:37 | test=— |          (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
display | crates/app/tests/extraction_fields.rs:73 | test=— |           text_value,approved_by_actor_kind,approved_by_actor_id)
storage | crates/app/tests/project_deletion.rs:101 | test=— | INSERT INTO studies (id, project_id, title, design_context, study_revision, updated_by_actor_kind, updated_by_actor_id) VALUES
storage | crates/app/tests/project_deletion.rs:106 | test=— | INSERT INTO study_events (id, project_id, study_id, event_type, before_revision, result_revision, actor_kind, actor_id, before_snapshot, result_snapshot, payload)
storage | crates/app/tests/project_deletion.rs:130 | test=— | INSERT INTO extraction_values (id, project_id, study_id, report_id, field_definition_id, field_definition_version, value_type, text_value, source_document_id, source_block_id, source_page, source_parser_version, source_content_hash, approved_by_actor_kind, approved_by_actor_id)
storage | crates/app/tests/project_deletion.rs:132 | test=— | INSERT INTO extraction_events (id, project_id, study_id, proposal_id, event_type, payload, actor_kind, actor_id)
storage | crates/app/tests/project_deletion.rs:134 | test=— | INSERT INTO screening_events (id, project_id, report_id, stage, protocol_version_id, actor_kind, actor_id, decision, result_final_status, result_title_abstract_status, result_full_text_status)
storage | crates/app/tests/project_deletion.rs:138 | test=— | INSERT INTO ai_activity (id, project_id, actor_type, actor_label, actor_kind, actor_id, task, action, summary, affected, evidence)
storage | crates/app/tests/project_deletion.rs:146 | test=— | INSERT INTO appraisal_assessments (id, project_id, report_id, definition_id, definition_version, responses, judgments, actor_kind, actor_id)
storage | crates/app/tests/project_deletion.rs:150 | test=— | INSERT INTO appraisal_events (id, assessment_id, project_id, report_id, event_type, payload, actor_kind, actor_id)
storage | crates/app/tests/project_deletion.rs:160 | test=— | INSERT INTO durable_runs (id, project_id, kind, actor_kind, actor_id, idempotency_key, status)
storage | crates/app/tests/project_deletion.rs:173 | test=— | INSERT INTO automation_definitions (id, project_id, name, trigger_kind, recipe_id, recipe_version, status, actor_kind, actor_id)
storage | crates/app/tests/project_deletion.rs:177 | test=— | INSERT INTO durable_runs (id, project_id, kind, job_id, actor_kind, actor_id, idempotency_key, status)
storage | crates/app/tests/project_deletion.rs:201 | test=— | INSERT INTO dedupe_resolution_events (id, project_id, record_id, action, reason, actor_kind, actor_id, proposal_id)
storage | crates/app/tests/protocol.rs:247 | test=— |     sqlx::query("INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id) VALUES ($1,$2,$3,'title_abstract','include',$4,'user','history-test')")
storage | crates/app/tests/protocol.rs:311 | test=— |     sqlx::query("INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id) VALUES ($1,$2,$3,'title_abstract','include',$4,'user','cascade-test')")
storage | crates/app/tests/protocol.rs:472 | test=— |         sqlx::query("INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id) VALUES ($1,$2,$3,'title_abstract','include',$4,'user','migration-test')")
display | crates/app/tests/review_runs.rs:1002 | test=— |             resolved_by_actor_id: None,
display | crates/app/tests/review_runs.rs:1310 | test=— |                 resolved_by_actor_id: None,
storage | crates/app/tests/screening.rs:566 | test=— |         sqlx::query("INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id,created_at) VALUES ($1,$2,$3,'title_abstract','include',$4,'user','legacy', $5)")
storage | crates/app/tests/screening.rs:575 | test=— |         sqlx::query("INSERT INTO screening_events (id,project_id,report_id,stage,decision,exclusion_reason_id,protocol_version_id,actor_kind,actor_id,created_at) VALUES ($1,$2,$3,'full_text','exclude',$4,$5,'user','legacy', $6)")
storage | crates/app/tests/screening.rs:585 | test=— |         sqlx::query("INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id,created_at) VALUES ($1,$2,$3,'title_abstract','maybe',$4,'user','legacy', $5)")
storage | crates/app/tests/screening.rs:740 | test=— |                 "INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id,origin)
display | crates/app/tests/study_ungrouped.rs:98 | test=— |          (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
display | crates/app/tests/workflow_review_counts.rs:58 | test=— |             actor_id: "review-counts".to_owned(),
```

## Verification and limitations

The DB tests verify old-function compatibility and rollback safety, all nine inventoried identity comparisons, two-user HTTP blinding, header/body forgery resistance, audited mapping/denial, UUID-shaped unresolved labels, byte-exact case/whitespace labels, mapping-aware reviewer distinctness at write/evaluate/finalize, unchanged legacy rows with provenance on new rows, and provenance after account deactivation. Development initialization also provisions the fixed auth-off principal only when the shared development gate is enabled, and its assignment write is covered.
