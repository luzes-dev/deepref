-- phase: expand
-- Pre-production fresh database baseline; replaces the unshipped migration chain.
-- Existing databases must be recreated, not upgraded across this reset.

--
-- PostgreSQL database dump
--


-- Dumped from database version 17.11 (Debian 17.11-1.pgdg12+2)
-- Dumped by pg_dump version 17.11 (Debian 17.11-1.pgdg12+2)

SET LOCAL statement_timeout = 0;
SET LOCAL lock_timeout = 0;
SET LOCAL idle_in_transaction_session_timeout = 0;
SET LOCAL transaction_timeout = 0;
SET LOCAL client_encoding = 'UTF8';
SET LOCAL standard_conforming_strings = on;
SET LOCAL check_function_bodies = false;
SET LOCAL xmloption = content;
SET LOCAL client_min_messages = warning;
SET LOCAL row_security = off;

--
-- Name: pg_trgm; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;


--
-- Name: EXTENSION pg_trgm; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON EXTENSION pg_trgm IS 'text similarity measurement and index searching based on trigrams';


--
-- Name: pgcrypto; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;


--
-- Name: EXTENSION pgcrypto; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON EXTENSION pgcrypto IS 'cryptographic functions';


--
-- Name: vector; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS vector WITH SCHEMA public;


--
-- Name: EXTENSION vector; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON EXTENSION vector IS 'vector data type and ivfflat and hnsw access methods';


--
-- Name: account_export_jobs_fence(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.account_export_jobs_fence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  declared_kinds text[];
BEGIN
  IF NEW.kind <> 'account_data_export' THEN
    RETURN NEW;
  END IF;
  declared_kinds := string_to_array(current_setting('deepref.job_kinds', true), ',');
  IF NOT coalesce(NEW.kind = ANY(declared_kinds), false) THEN
    IF NEW.state = 'running' AND (TG_OP = 'INSERT' OR OLD.state <> 'running') THEN
      RETURN NULL;
    END IF;
    IF TG_OP = 'UPDATE' AND OLD.state = 'running' AND OLD.leased_until < now() THEN
      RETURN NULL;
    END IF;
  END IF;
  IF NEW.state = 'queued' THEN
    NEW.state := 'capability_wait';
  END IF;
  RETURN NEW;
END;
$$;


--
-- Name: account_export_transition_guard(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.account_export_transition_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.user_id <> OLD.user_id OR NEW.job_id <> OLD.job_id THEN
    RAISE EXCEPTION 'export owner and durable job are immutable';
  END IF;
  IF OLD.status IN ('failed','cancelled','expired') AND NEW.status <> OLD.status THEN
    RAISE EXCEPTION 'released export cannot be republished';
  END IF;
  IF OLD.status = 'completed' AND NEW.status NOT IN ('completed','failed','cancelled','expired') THEN
    RAISE EXCEPTION 'completed export cannot restart';
  END IF;
  IF OLD.status = 'processing' AND NEW.status = 'processing'
     AND (NEW.claim_owner,NEW.claim_seq) IS DISTINCT FROM (OLD.claim_owner,OLD.claim_seq)
     AND NEW.attempt_id IS NOT DISTINCT FROM OLD.attempt_id THEN
    RAISE EXCEPTION 'replacement claim requires a fresh attempt';
  END IF;
  RETURN NEW;
END;
$$;


--
-- Name: activate_account_erasure(text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.activate_account_erasure(p_attestation text) RETURNS boolean
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'pg_catalog', 'public'
    SET lock_timeout TO '250ms'
    SET statement_timeout TO '1500ms'
    AS $$
BEGIN
  IF p_attestation IS DISTINCT FROM 'all-legacy-export-writers-stopped-and-untracked-staging-removed-no-restart' THEN
    RAISE EXCEPTION 'exact legacy writer/staging cleanup attestation required';
  END IF;
  -- Older mutable request pointers cannot establish the contents of every
  -- historical archive. Require actual deletion ACKs for all unjournaled
  -- account objects, including replaced attempts, before enabling proofs.
  IF EXISTS(SELECT 1 FROM public.object_lifecycle o
      WHERE o.user_id IS NOT NULL AND o.deleted_at IS NULL
        AND NOT EXISTS(SELECT 1 FROM public.account_export_writers w
          WHERE w.archive_key=o.storage_key)) THEN
    RAISE EXCEPTION 'unjournaled legacy account objects still require deletion acknowledgements';
  END IF;
  UPDATE public.account_erasure_rollout SET activated=true,
    activated_at=coalesce(activated_at,clock_timestamp()),attestation=p_attestation
    WHERE singleton;
  RETURN FOUND;
END $$;


--
-- Name: activate_export_retention(text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.activate_export_retention(p_attestation text) RETURNS boolean
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'pg_catalog'
    SET lock_timeout TO '250ms'
    SET statement_timeout TO '1500ms'
    AS $$
BEGIN
  IF p_attestation IS DISTINCT FROM 'all-eager-deleting-binaries-drained-no-restart' THEN
    RAISE EXCEPTION 'exact eager-deleter drain and no-restart attestation required'
      USING ERRCODE='invalid_parameter_value';
  END IF;
  UPDATE public.account_export_rollout
    SET retention_activated=true,activated_at=coalesce(activated_at,clock_timestamp()),
        operator_attestation=p_attestation WHERE singleton;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'export retention rollout control is missing'
      USING ERRCODE='integrity_constraint_violation';
  END IF;
  RETURN true;
END;
$$;


--
-- Name: actor_identity_may_be_user(text, text, uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.actor_identity_may_be_user(p_actor_kind text, p_actor_id text, p_actor_user_id uuid, p_user_id uuid) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
  WITH identity AS (
    SELECT resolved_human_actor_user_id(
      p_actor_kind, p_actor_id, p_actor_user_id
    ) AS actor_user_id
  )
  SELECT COALESCE(p_actor_kind = 'user'
    AND (
      i.actor_user_id = p_user_id
      OR (
        i.actor_user_id IS NULL
        AND legacy_human_actor_is_unmapped(p_actor_kind, p_actor_id)
        AND NOT EXISTS (
          SELECT 1 FROM legacy_actor_non_identities n
          WHERE n.legacy_actor_kind = p_actor_kind
            AND n.legacy_actor_id = p_actor_id
            AND n.user_id = p_user_id
        )
      )
    ), false)
  FROM identity i
$$;


--
-- Name: ai_first_audit_has_two_distinct_reviewers(uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_audit_has_two_distinct_reviewers(p_cohort uuid, p_report uuid) RETURNS boolean
    LANGUAGE sql
    AS $$
  WITH identity_lock AS MATERIALIZED (
    SELECT pg_advisory_xact_lock_shared(73003203)
  ), labels AS (
    SELECT resolved_human_actor_user_id(
             'user', l.actor_id, l.actor_user_id
           ) AS user_id
    FROM ai_screening_audit_labels l
    WHERE l.cohort_id = p_cohort AND l.report_id = p_report
  )
  SELECT count(*) = 2
    AND count(user_id) = 2
    AND count(DISTINCT user_id) = 2
  FROM labels CROSS JOIN identity_lock
$$;


--
-- Name: ai_first_audit_masked(uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_audit_masked(project uuid, report uuid) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
 SELECT EXISTS(SELECT 1 FROM ai_screening_cohort_members m
 JOIN ai_screening_cohorts c ON c.id=m.cohort_id
 WHERE m.project_id=project AND m.report_id=report AND c.status='auditing')
$$;


--
-- Name: ai_first_blinded(uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_blinded(project uuid, report uuid) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
 SELECT EXISTS(SELECT 1 FROM ai_screening_cohort_members m
  JOIN ai_screening_cohorts c ON c.id=m.cohort_id
  WHERE m.project_id=project AND m.report_id=report
    AND c.status IN ('open','closed','auditing','passed'))
$$;


--
-- Name: ai_first_fresh_auditor(uuid, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_fresh_auditor(cohort uuid, actor text) RETURNS boolean
    LANGUAGE sql
    AS $$
  WITH identity_lock AS MATERIALIZED (
    SELECT pg_advisory_xact_lock_shared(73003203)
  ), scope AS (
    SELECT c.id, c.project_id,
           c.approved_by, c.approved_by_actor_user_id,
           c.finalized_by, c.finalized_by_actor_user_id
    FROM ai_screening_cohorts c WHERE c.id = cohort
  ), history AS (
    SELECT 'user'::text AS actor_kind, s.approved_by AS actor_id,
           s.approved_by_actor_user_id AS actor_user_id FROM scope s
    UNION ALL
    SELECT 'user'::text, s.finalized_by, s.finalized_by_actor_user_id
    FROM scope s WHERE s.finalized_by IS NOT NULL
    UNION ALL
    SELECT 'user'::text, a.approved_by, a.approved_by_actor_user_id
    FROM scope s JOIN project_ai_screening_authority a ON a.project_id = s.project_id
    UNION ALL
    SELECT e.actor_kind, e.actor_id, e.actor_user_id
    FROM scope s
    JOIN ai_screening_cohort_members m ON m.cohort_id = s.id
    JOIN screening_events e
      ON e.project_id = m.project_id AND e.report_id = m.report_id
    WHERE e.actor_kind = 'user'
    UNION ALL
    SELECT e.audience_actor_kind, e.audience_actor_id, e.audience_actor_user_id
    FROM scope s
    JOIN ai_screening_cohort_members m ON m.cohort_id = s.id
    JOIN ai_opinion_exposures e
      ON e.project_id = m.project_id AND e.report_id = m.report_id
    WHERE e.audience_actor_id IS NOT NULL
    UNION ALL
    SELECT 'user'::text, l.actor_id, l.actor_user_id
    FROM scope s JOIN ai_screening_audit_labels l ON l.cohort_id = s.id
  )
  SELECT actor IS NOT NULL AND EXISTS (
    SELECT 1 FROM scope s
    WHERE NOT EXISTS (
      SELECT 1 FROM history h
      WHERE same_human_actor_identity(
        'user', actor, NULL::uuid,
        h.actor_kind, h.actor_id, h.actor_user_id
      )
    )
      AND NOT EXISTS (
        SELECT 1 FROM ai_screening_cohort_members m
        JOIN ai_opinion_exposures e
          ON e.project_id = m.project_id AND e.report_id = m.report_id
        WHERE m.cohort_id = s.id AND e.audience_actor_id IS NULL
      )
  )
  FROM identity_lock
$$;


--
-- Name: ai_first_fresh_auditor_v2(uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_fresh_auditor_v2(p_cohort uuid, p_user_id uuid) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$ SELECT false $$;


--
-- Name: ai_first_fresh_auditor_v3(uuid, uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_fresh_auditor_v3(p_cohort uuid, p_report uuid, p_user_id uuid) RETURNS boolean
    LANGUAGE sql
    AS $$
  WITH identity_lock AS MATERIALIZED (
    SELECT pg_advisory_xact_lock_shared(73003203)
  )
  SELECT EXISTS (
    SELECT 1 FROM ai_screening_cohorts c
    WHERE c.id = p_cohort
      AND ai_first_human_history_clear(c.id, p_user_id)
      AND EXISTS (
        SELECT 1 FROM scientific_assignments a
        WHERE a.project_id = c.project_id
          AND a.user_id = p_user_id
          AND a.role = 'fresh_ai_auditor'
          AND a.revoked_at IS NULL
      )
      AND NOT EXISTS (
        SELECT 1 FROM ai_screening_audit_labels l
        WHERE l.cohort_id = c.id AND l.report_id = p_report
          AND same_human_actor_identity(
            'user', p_user_id::text, p_user_id,
            'user', l.actor_id, l.actor_user_id
          )
      )
  )
  FROM identity_lock
$$;


--
-- Name: ai_first_human_history_clear(uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_human_history_clear(p_cohort uuid, p_user_id uuid) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
  WITH scope AS (
    SELECT c.id, c.project_id,
           c.approved_by, c.approved_by_actor_user_id,
           c.finalized_by, c.finalized_by_actor_user_id
    FROM ai_screening_cohorts c WHERE c.id = p_cohort
  ), history AS (
    SELECT 'user'::text AS actor_kind, s.approved_by AS actor_id,
           s.approved_by_actor_user_id AS actor_user_id FROM scope s
    UNION ALL
    SELECT 'user'::text, s.finalized_by, s.finalized_by_actor_user_id
    FROM scope s WHERE s.finalized_by IS NOT NULL
    UNION ALL
    SELECT 'user'::text, a.approved_by, a.approved_by_actor_user_id
    FROM scope s JOIN project_ai_screening_authority a ON a.project_id = s.project_id
    UNION ALL
    SELECT e.actor_kind, e.actor_id, e.actor_user_id
    FROM scope s
    JOIN ai_screening_cohort_members m ON m.cohort_id = s.id
    JOIN screening_events e
      ON e.project_id = m.project_id AND e.report_id = m.report_id
    WHERE e.actor_kind = 'user'
    UNION ALL
    SELECT e.audience_actor_kind, e.audience_actor_id, e.audience_actor_user_id
    FROM scope s
    JOIN ai_screening_cohort_members m ON m.cohort_id = s.id
    JOIN ai_opinion_exposures e
      ON e.project_id = m.project_id AND e.report_id = m.report_id
    WHERE e.audience_actor_kind = 'user' AND e.audience_actor_id IS NOT NULL
  )
  SELECT EXISTS (
    SELECT 1 FROM scope s
    WHERE NOT EXISTS (
      SELECT 1 FROM history h
      WHERE same_human_actor_identity(
        'user', p_user_id::text, p_user_id,
        h.actor_kind, h.actor_id, h.actor_user_id
      )
    )
      AND NOT EXISTS (
        SELECT 1 FROM ai_screening_cohort_members m
        JOIN ai_opinion_exposures e
          ON e.project_id = m.project_id AND e.report_id = m.report_id
        WHERE m.cohort_id = s.id AND e.audience_actor_id IS NULL
      )
  )
$$;


--
-- Name: ai_first_queue_time(uuid, uuid, timestamp with time zone); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_queue_time(project uuid, report uuid, original timestamp with time zone) RETURNS timestamp with time zone
    LANGUAGE sql STABLE
    AS $$
 SELECT COALESCE((SELECT '2000-01-01 UTC'::timestamptz + m.audit_order * interval '1 microsecond'
 FROM ai_screening_cohort_members m JOIN ai_screening_cohorts c ON c.id=m.cohort_id
 WHERE m.project_id=project AND m.report_id=report AND c.status='auditing'),original)
$$;


--
-- Name: ai_first_queue_visible(uuid, uuid, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_queue_visible(project uuid, report uuid, actor text) RETURNS boolean
    LANGUAGE sql
    AS $$
  WITH identity_lock AS MATERIALIZED (
    SELECT pg_advisory_xact_lock_shared(73003203)
  )
  SELECT CASE
    WHEN ai_first_audit_masked(project, report) THEN EXISTS (
      SELECT 1 FROM ai_screening_cohort_members m
      JOIN ai_screening_cohorts c ON c.id = m.cohort_id
      WHERE m.project_id = project AND m.report_id = report
        AND c.status = 'auditing' AND (m.sampled OR m.audit_control)
        AND ai_first_fresh_auditor(c.id, actor)
        AND NOT EXISTS (
          SELECT 1 FROM ai_screening_audit_labels l
          WHERE l.cohort_id = c.id AND l.report_id = report
            AND same_human_actor_identity(
              'user', actor, NULL::uuid,
              'user', l.actor_id, l.actor_user_id
            )
        )
        AND (SELECT count(*) FROM ai_screening_audit_labels l
             WHERE l.cohort_id = c.id AND l.report_id = report) < 2
    )
    ELSE NOT EXISTS (
      SELECT 1 FROM ai_screening_dispositions d
      JOIN ai_screening_cohorts c ON c.id = d.cohort_id
      WHERE d.project_id = project AND d.report_id = report
        AND d.voided_at IS NULL AND d.finalized_at IS NULL
        AND c.status IN ('open', 'closed', 'passed')
    )
  END
  FROM identity_lock
$$;


--
-- Name: ai_first_queue_visible_v2(uuid, uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_queue_visible_v2(p_project uuid, p_report uuid, p_user_id uuid) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$ SELECT false $$;


--
-- Name: ai_first_queue_visible_v3(uuid, uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_queue_visible_v3(p_project uuid, p_report uuid, p_user_id uuid) RETURNS boolean
    LANGUAGE sql
    AS $$
  WITH identity_lock AS MATERIALIZED (
    SELECT pg_advisory_xact_lock_shared(73003203)
  )
  SELECT CASE
    WHEN ai_first_audit_masked(p_project, p_report) THEN EXISTS (
      SELECT 1 FROM ai_screening_cohort_members m
      JOIN ai_screening_cohorts c ON c.id = m.cohort_id
      WHERE m.project_id = p_project AND m.report_id = p_report
        AND c.status = 'auditing' AND (m.sampled OR m.audit_control)
        AND ai_first_fresh_auditor_v3(c.id, p_report, p_user_id)
        AND (SELECT count(*) FROM ai_screening_audit_labels l
             WHERE l.cohort_id = c.id AND l.report_id = p_report) < 2
    )
    ELSE NOT EXISTS (
      SELECT 1 FROM ai_screening_dispositions d
      JOIN ai_screening_cohorts c ON c.id = d.cohort_id
      WHERE d.project_id = p_project AND d.report_id = p_report
        AND d.voided_at IS NULL AND d.finalized_at IS NULL
        AND c.status IN ('open', 'closed', 'passed')
    )
  END
  FROM identity_lock
$$;


--
-- Name: ai_first_run_visible(uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_run_visible(project uuid, run uuid) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
 SELECT ai_first_run_visible_legacy(project,run)
 AND NOT EXISTS(SELECT 1 FROM ai_screening_cohorts c WHERE c.project_id=project AND c.status='auditing')
 AND NOT EXISTS(SELECT 1 FROM review_run_manifests r
 JOIN ai_screening_cohort_members m ON m.project_id=r.project_id AND m.report_id::text=r.subject->>'report_id'
 JOIN ai_screening_cohorts c ON c.id=m.cohort_id
 WHERE r.project_id=project AND r.automation_run_id=run AND c.status IN ('open','closed','auditing','passed'))
$$;


--
-- Name: ai_first_run_visible_legacy(uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_first_run_visible_legacy(project uuid, run uuid) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
 SELECT NOT EXISTS(SELECT 1 FROM review_run_manifests m
  JOIN ai_screening_cohorts c ON c.id=(m.origin->>'cohort_id')::uuid
  WHERE m.project_id=project AND m.automation_run_id=run AND m.origin->>'kind'='ai_first_triggered'
    AND c.status IN ('open','closed','auditing','passed'))
$$;


--
-- Name: ai_screening_proposal_withheld(uuid, uuid, uuid, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_screening_proposal_withheld(project uuid, proposal uuid, report uuid, screening_stage text) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
 SELECT (screening_stage='title_abstract' AND ai_first_blinded(project,report))
   OR ai_screening_proposal_withheld_legacy(project,proposal,report,screening_stage)
$$;


--
-- Name: ai_screening_proposal_withheld_legacy(uuid, uuid, uuid, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_screening_proposal_withheld_legacy(project uuid, proposal uuid, report uuid, screening_stage text) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
    SELECT COALESCE(screening_stage IN ('title_abstract', 'full_text'), false) AND (
      EXISTS (
        SELECT 1 FROM ai_reviewer_decisions d
        LEFT JOIN screening_state s ON s.project_id=d.project_id AND s.report_id=d.report_id
        WHERE d.project_id=project AND d.proposal_id=proposal AND d.resolved_at IS NULL
          AND COALESCE(CASE WHEN d.stage='full_text' THEN s.full_text_status
                           ELSE s.title_abstract_status END, 'unscreened')
              IN ('unscreened', 'not_required')
      ) OR (
        COALESCE((SELECT a.level FROM project_ai_autonomy a
                  WHERE a.project_id=project AND a.task=screening_stage || '_screening'),
                 'second_reviewer')='second_reviewer'
        AND COALESCE((SELECT CASE WHEN screening_stage='full_text' THEN s.full_text_status
                                  ELSE s.title_abstract_status END
                      FROM screening_state s WHERE s.project_id=project AND s.report_id=report),
                     'unscreened') IN ('unscreened', 'not_required')
      )
    )
$$;


--
-- Name: ai_usage_jobs_fence(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_usage_jobs_fence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  declared_kinds text[];
BEGIN
  IF NEW.kind NOT IN ('ai_usage_reconcile', 'ai_usage_export_replay') THEN
    RETURN NEW;
  END IF;
  declared_kinds := string_to_array(current_setting('deepref.job_kinds', true), ',');
  IF NOT coalesce(NEW.kind = ANY(declared_kinds), false) THEN
    IF NEW.state = 'running' AND (TG_OP = 'INSERT' OR OLD.state <> 'running') THEN
      RETURN NULL;
    END IF;
    -- N-1 lease recovery decodes every UPDATE RETURNING kind too. Leave an
    -- expired protected lease for capable recovery, returning no unknown row.
    IF TG_OP = 'UPDATE' AND OLD.state = 'running' AND OLD.leased_until < now() THEN
      RETURN NULL;
    END IF;
  END IF;
  -- N-1 selects queued rows BEFORE LIMIT. Keep protected pending work outside
  -- that candidate set altogether, so a protected row cannot starve ordinary
  -- work in an all-N-1 fleet. Only N also selects capability_wait.
  IF NEW.state = 'queued' THEN
    NEW.state := 'capability_wait';
  END IF;
  IF NEW.kind = 'ai_usage_export_replay' AND NEW.state = 'capability_wait'
     AND NOT coalesce((SELECT ai_usage_export_activated
                       FROM ai_usage_rollout_state WHERE singleton), false) THEN
    NEW.state := 'failed';
    NEW.last_error := 'parked by ai-usage disable-export (database fence)';
    NEW.lease_owner := NULL;
    NEW.leased_until := NULL;
    NEW.lease_renewed_at := NULL;
  END IF;
  -- Recovery remains available when admission caps are suspended. N-1 cannot
  -- claim it, and a capable N recovery worker can drain retained liabilities.
  RETURN NEW;
END;
$$;


--
-- Name: ai_usage_jobs_write_lock(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_usage_jobs_write_lock() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  PERFORM pg_advisory_xact_lock_shared(913, 9);
  RETURN NULL;
END;
$$;


--
-- Name: ai_usage_ledger_count_legacy_spend(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_usage_ledger_count_legacy_spend() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  v_workspace_id uuid;
  v_project_workspace_id uuid;
  v_period date;
  v_platform_limit bigint;
  v_workspace_limit bigint;
  v_project_limit bigint;
  v_prior_overage bigint;
  v_overage bigint;
BEGIN
  IF NEW.reservation_id IS NOT NULL THEN
    RETURN NULL;
  END IF;
  UPDATE ai_usage_rollout_state
  SET last_legacy_ledger_insert_at = now(), updated_at = now()
  WHERE singleton;
  -- A legacy budget write can already own this row. Lock it before any
  -- hierarchy bucket so it follows project → platform → workspace → project.
  IF NEW.project_id IS NOT NULL THEN
    SELECT p.workspace_id INTO v_project_workspace_id FROM projects p
    WHERE p.id = NEW.project_id FOR NO KEY UPDATE;
  END IF;
  -- N-1 writes only legacy ledger columns. Prefer the locked current tenant;
  -- the fallback covers ledger rows whose project has already been removed.
  v_workspace_id := COALESCE(v_project_workspace_id, NEW.workspace_id);
  v_period := COALESCE(NEW.period_start, date_trunc('month', NEW.created_at AT TIME ZONE 'UTC')::date);
  INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, period_start, limit_micros)
  SELECT 'platform', scope_id, v_period, limit_micros FROM ai_usage_limits
  WHERE scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001'
  ON CONFLICT DO NOTHING;
  SELECT limit_micros INTO v_platform_limit FROM ai_usage_period_buckets
    WHERE scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001'
      AND period_start = v_period FOR UPDATE;
  IF v_platform_limit IS NULL THEN
    RAISE EXCEPTION 'AI platform usage bucket is missing for legacy spend';
  END IF;
  IF v_workspace_id IS NOT NULL THEN
    INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, period_start, limit_micros)
    SELECT 'workspace', scope_id, scope_id, v_period, limit_micros FROM ai_usage_limits
    WHERE scope_kind = 'workspace' AND scope_id = v_workspace_id
    ON CONFLICT DO NOTHING;
    SELECT limit_micros INTO v_workspace_limit FROM ai_usage_period_buckets
      WHERE scope_kind = 'workspace' AND scope_id = v_workspace_id AND period_start = v_period FOR UPDATE;
    IF v_workspace_limit IS NULL THEN
      RAISE EXCEPTION 'AI workspace usage bucket is missing for legacy spend';
    END IF;
    INSERT INTO ai_usage_workspace_admission_state (workspace_id) VALUES (v_workspace_id)
      ON CONFLICT DO NOTHING;
    PERFORM 1 FROM ai_usage_workspace_admission_state
      WHERE workspace_id = v_workspace_id FOR UPDATE;
  END IF;
  IF v_workspace_id IS NOT NULL AND NEW.project_id IS NOT NULL THEN
    INSERT INTO ai_usage_period_buckets
      (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros)
    SELECT 'project', scope_id, v_workspace_id, scope_id, v_period, limit_micros FROM ai_usage_limits
    WHERE scope_kind = 'project' AND scope_id = NEW.project_id
    ON CONFLICT DO NOTHING;
    SELECT limit_micros INTO v_project_limit FROM ai_usage_period_buckets
      WHERE scope_kind = 'project' AND scope_id = NEW.project_id AND period_start = v_period FOR UPDATE;
    IF v_project_limit IS NULL THEN
      RAISE EXCEPTION 'AI project usage bucket is missing for legacy spend';
    END IF;
  END IF;
  SELECT COALESCE(MAX(GREATEST(0, b.spent_micros + b.reserved_micros - b.limit_micros)), 0)
  INTO v_prior_overage
  FROM ai_usage_period_buckets b
  WHERE b.period_start = v_period
    AND ((b.scope_kind = 'platform' AND b.scope_id = '00000000-0000-0000-0000-000000000001'::uuid)
      OR (v_workspace_id IS NOT NULL AND b.scope_kind = 'workspace' AND b.scope_id = v_workspace_id)
      OR (v_workspace_id IS NOT NULL AND NEW.project_id IS NOT NULL
          AND b.scope_kind = 'project' AND b.scope_id = NEW.project_id));
  UPDATE ai_usage_period_buckets SET spent_micros = spent_micros + NEW.cost_micros, updated_at = now()
    WHERE scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001'
      AND period_start = v_period;
  IF v_workspace_id IS NOT NULL THEN
    UPDATE ai_usage_period_buckets SET spent_micros = spent_micros + NEW.cost_micros, updated_at = now()
      WHERE scope_kind = 'workspace' AND scope_id = v_workspace_id AND period_start = v_period;
  END IF;
  IF v_workspace_id IS NOT NULL AND NEW.project_id IS NOT NULL THEN
    UPDATE ai_usage_period_buckets SET spent_micros = spent_micros + NEW.cost_micros, updated_at = now()
      WHERE scope_kind = 'project' AND scope_id = NEW.project_id AND period_start = v_period;
  END IF;
  INSERT INTO ai_usage_legacy_backfill (ledger_id) VALUES (NEW.id) ON CONFLICT DO NOTHING;
  SELECT COALESCE(MAX(GREATEST(0, b.spent_micros + b.reserved_micros - b.limit_micros)), 0)
  INTO v_overage
  FROM ai_usage_period_buckets b
  WHERE b.period_start = v_period
    AND ((b.scope_kind = 'platform' AND b.scope_id = '00000000-0000-0000-0000-000000000001'::uuid)
      OR (v_workspace_id IS NOT NULL AND b.scope_kind = 'workspace' AND b.scope_id = v_workspace_id)
      OR (v_workspace_id IS NOT NULL AND NEW.project_id IS NOT NULL
          AND b.scope_kind = 'project' AND b.scope_id = NEW.project_id));
  v_overage := GREATEST(0, v_overage - v_prior_overage);
  IF v_overage > 0 AND v_workspace_id IS NOT NULL THEN
    UPDATE ai_usage_workspace_admission_state SET blocked = true, updated_at = now()
      WHERE workspace_id = v_workspace_id;
    INSERT INTO ai_usage_workspace_blocks (workspace_id, overage_micros, reservation_id)
    VALUES (v_workspace_id, v_overage, COALESCE(NEW.reservation_id, NEW.id))
    ON CONFLICT (workspace_id) DO UPDATE
      SET overage_micros = ai_usage_workspace_blocks.overage_micros + EXCLUDED.overage_micros,
          reservation_id = EXCLUDED.reservation_id,
          blocked_at = LEAST(ai_usage_workspace_blocks.blocked_at, now()),
          cleared_at = NULL, cleared_by_user_id = NULL, clear_reason = NULL;
  END IF;
  RETURN NULL;
END;
$$;


--
-- Name: ai_usage_ledger_fill_legacy_identity(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_usage_ledger_fill_legacy_identity() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.period_start IS NULL THEN
    NEW.period_start := date_trunc('month', NEW.created_at AT TIME ZONE 'UTC')::date;
  END IF;
  IF NEW.workspace_id IS NULL AND NEW.project_id IS NOT NULL THEN
    SELECT p.workspace_id INTO NEW.workspace_id FROM projects p WHERE p.id = NEW.project_id;
  END IF;
  IF NEW.run_budget_id IS NULL THEN
    NEW.run_budget_id := NEW.run_id;
  END IF;
  RETURN NEW;
END;
$$;


--
-- Name: ai_usage_register_fact_id(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_usage_register_fact_id() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  v_kind text := TG_ARGV[0];
BEGIN
  IF TG_OP = 'UPDATE' THEN
    RAISE EXCEPTION 'ai_usage fact ids are immutable (%, id %)', TG_TABLE_NAME, NEW.id;
  END IF;
  INSERT INTO ai_usage_fact_ids (fact_id, fact_kind) VALUES (NEW.id, v_kind);
  RETURN NEW;
EXCEPTION WHEN unique_violation THEN
  -- Re-registering the same fact row is harmless: that row's own primary key
  -- decides the insert. A different fact kind is a namespace collision.
  IF EXISTS (
    SELECT 1 FROM ai_usage_fact_ids WHERE fact_id = NEW.id AND fact_kind = v_kind
  ) THEN
    RETURN NEW;
  END IF;
  RAISE EXCEPTION 'ai_usage fact id % is already used by another fact record', NEW.id;
END;
$$;


--
-- Name: ai_usage_sync_new_project_limit(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ai_usage_sync_new_project_limit() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  v_period date := date_trunc('month', now() AT TIME ZONE 'UTC')::date;
  v_spent bigint;
  v_reserved bigint;
BEGIN
  -- INSERT/UPDATE has already locked this tuple. State the lock contract
  -- explicitly before any hierarchy bucket access so project cap writes and
  -- legacy ledger writes cannot invert project → bucket ordering.
  IF TG_OP <> 'INSERT' THEN
    PERFORM 1 FROM projects WHERE id = NEW.id FOR NO KEY UPDATE;
  END IF;

  IF TG_OP = 'INSERT' THEN
    INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
    VALUES ('project', NEW.id, NEW.ai_monthly_budget_micros) ON CONFLICT DO NOTHING;
    IF NEW.workspace_id IS NULL THEN
      RETURN NEW;
    END IF;
    INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
    VALUES ('workspace', NEW.workspace_id, 0) ON CONFLICT DO NOTHING;
    INSERT INTO ai_usage_period_buckets
      (scope_kind, scope_id, workspace_id, period_start, limit_micros)
    SELECT 'workspace', NEW.workspace_id, NEW.workspace_id, v_period, limit_micros
    FROM ai_usage_limits WHERE scope_kind = 'workspace' AND scope_id = NEW.workspace_id
    ON CONFLICT DO NOTHING;
    INSERT INTO ai_usage_period_buckets
      (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros)
    VALUES ('project', NEW.id, NEW.workspace_id, NEW.id, v_period, NEW.ai_monthly_budget_micros)
    ON CONFLICT DO NOTHING;
    RETURN NEW;
  END IF;

  IF NEW.workspace_id IS NULL THEN
    RETURN NEW;
  END IF;
  -- The project row is locked before platform → workspace → project buckets.
  -- Assigning a workspace to an old workspace-less project creates its
  -- hierarchy lazily; a child update never changes parent allowance.
  INSERT INTO ai_usage_limits (scope_kind, scope_id, limit_micros)
  VALUES ('workspace', NEW.workspace_id, 0) ON CONFLICT DO NOTHING;
  INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, period_start, limit_micros)
  SELECT 'platform', scope_id, v_period, limit_micros FROM ai_usage_limits
  WHERE scope_kind = 'platform' AND scope_id = '00000000-0000-0000-0000-000000000001'
  ON CONFLICT DO NOTHING;
  INSERT INTO ai_usage_period_buckets (scope_kind, scope_id, workspace_id, period_start, limit_micros)
  SELECT 'workspace', scope_id, scope_id, v_period, limit_micros FROM ai_usage_limits
  WHERE scope_kind = 'workspace' AND scope_id = NEW.workspace_id
  ON CONFLICT DO NOTHING;
  INSERT INTO ai_usage_period_buckets
    (scope_kind, scope_id, workspace_id, project_id, period_start, limit_micros)
  SELECT 'project', scope_id, NEW.workspace_id, scope_id, v_period, limit_micros
  FROM ai_usage_limits WHERE scope_kind = 'project' AND scope_id = NEW.id
  ON CONFLICT DO NOTHING;
  PERFORM 1 FROM ai_usage_period_buckets WHERE scope_kind = 'platform'
    AND scope_id = '00000000-0000-0000-0000-000000000001' AND period_start = v_period FOR UPDATE;
  PERFORM 1 FROM ai_usage_period_buckets WHERE scope_kind = 'workspace'
    AND scope_id = NEW.workspace_id AND period_start = v_period FOR UPDATE;
  PERFORM 1 FROM ai_usage_period_buckets WHERE scope_kind = 'project'
    AND scope_id = NEW.id AND period_start = v_period FOR UPDATE;
  SELECT spent_micros, reserved_micros INTO v_spent, v_reserved
    FROM ai_usage_period_buckets WHERE scope_kind = 'project'
      AND scope_id = NEW.id AND period_start = v_period;
  IF v_spent + v_reserved > NEW.ai_monthly_budget_micros THEN
    RAISE EXCEPTION 'AI project cap is below its current spend and reservations';
  END IF;
  UPDATE ai_usage_limits SET limit_micros = NEW.ai_monthly_budget_micros, updated_at = now()
    WHERE scope_kind = 'project' AND scope_id = NEW.id;
  UPDATE ai_usage_period_buckets SET limit_micros = NEW.ai_monthly_budget_micros, updated_at = now()
    WHERE scope_kind = 'project' AND scope_id = NEW.id AND period_start = v_period;
  RETURN NEW;
END;
$$;


--
-- Name: assign_project_workspace_default(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.assign_project_workspace_default() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  NEW.workspace_id := 'f45d8021-6271-556e-a59a-fd2e0a739f95';
  RETURN NEW;
END;
$$;


--
-- Name: automation_builtin_recipe_steps(text, integer); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.automation_builtin_recipe_steps(target_recipe_id text, target_recipe_version integer) RETURNS TABLE(ordinal integer, step_key text, step_kind text)
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    AS $$
  SELECT recipe.ordinal, recipe.step_key, recipe.step_kind
  FROM (VALUES
    ('project_maintenance', 1, 0, 'recompute_project_metrics', 'deterministic_action'),
    ('review_screening', 1, 0, 'execute_compiled_review', 'ai_task'),
    ('review_duplicate_detection', 1, 0, 'execute_compiled_review', 'ai_task'),
    ('review_study_classification', 1, 0, 'execute_compiled_review', 'ai_task'),
    ('review_study_grouping', 1, 0, 'execute_compiled_review', 'ai_task'),
    ('review_appraisal_prefill', 1, 0, 'execute_compiled_review', 'ai_task'),
    ('review_data_extraction', 1, 0, 'execute_compiled_review', 'ai_task')
  ) AS recipe(recipe_id, recipe_version, ordinal, step_key, step_kind)
  WHERE recipe.recipe_id = target_recipe_id
    AND recipe.recipe_version = target_recipe_version
$$;


--
-- Name: automation_uuid_from_text(text, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.automation_uuid_from_text(prefix text, identity text) RETURNS uuid
    LANGUAGE sql IMMUTABLE
    AS $$
  SELECT format(
    '%s-%s-%s-%s-%s',
    substr(md5(prefix || ':' || identity), 1, 8),
    substr(md5(prefix || ':' || identity), 9, 4),
    substr(md5(prefix || ':' || identity), 13, 4),
    substr(md5(prefix || ':' || identity), 17, 4),
    substr(md5(prefix || ':' || identity), 21, 12)
  )::uuid
$$;


--
-- Name: backfill_project_workspaces_batch(integer); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.backfill_project_workspaces_batch(batch_size integer DEFAULT 500) RETURNS integer
    LANGUAGE plpgsql
    AS $$
DECLARE
  moved integer;
BEGIN
  IF batch_size IS NULL OR batch_size < 1 THEN
    RAISE EXCEPTION 'batch_size must be at least 1';
  END IF;
  UPDATE projects
  SET workspace_id = 'f45d8021-6271-556e-a59a-fd2e0a739f95'
  WHERE id IN (
    SELECT id FROM projects WHERE workspace_id IS NULL ORDER BY id LIMIT batch_size
  );
  GET DIAGNOSTICS moved = ROW_COUNT;
  RETURN moved;
END;
$$;


--
-- Name: billing_detach_workspace(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.billing_detach_workspace() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  UPDATE billing_accounts SET workspace_id=NULL, customer_ref=NULL, subscription_ref=NULL, product_ref=NULL,
    generation=generation+1, provisioning='detached', detached_at=now(),
    purge_after=now()+interval '5 years', projection='{"kind":"unknown"}',
    review_required=true
    WHERE workspace_id=OLD.id;
  RETURN OLD;
END $$;


--
-- Name: billing_preserve_closure_context(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.billing_preserve_closure_context() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'pg_catalog', 'public'
    AS $$
BEGIN
  IF TG_OP='INSERT' THEN
    IF NEW.closure_captured_at IS NOT NULL OR NEW.closure_workspace_id IS NOT NULL
      OR NEW.closure_customer_ref IS NOT NULL OR NEW.closure_subscription_ref IS NOT NULL THEN
      RAISE EXCEPTION 'closure context must come from original active mapping'
        USING ERRCODE='23514';
    END IF;
    RETURN NEW;
  END IF;

  IF OLD.closure_captured_at IS NOT NULL THEN
    IF NEW.closure_captured_at IS DISTINCT FROM OLD.closure_captured_at
      OR NEW.closure_workspace_id IS DISTINCT FROM OLD.closure_workspace_id
      OR NEW.closure_customer_ref IS DISTINCT FROM OLD.closure_customer_ref
      OR NEW.closure_subscription_ref IS DISTINCT FROM OLD.closure_subscription_ref
      OR NEW.provider_scope IS DISTINCT FROM OLD.provider_scope THEN
      RAISE EXCEPTION 'original closure context is immutable'
        USING ERRCODE='23514';
    END IF;
  ELSIF OLD.provisioning<>'detached' AND NEW.provisioning='detached' THEN
    IF NEW.provider_scope IS DISTINCT FROM OLD.provider_scope THEN
      RAISE EXCEPTION 'original closure scope cannot change during detachment'
        USING ERRCODE='23514';
    END IF;
    NEW.closure_workspace_id := OLD.workspace_id;
    NEW.closure_customer_ref := OLD.customer_ref;
    NEW.closure_subscription_ref := OLD.subscription_ref;
    NEW.closure_captured_at := clock_timestamp();
  ELSIF NEW.closure_captured_at IS NOT NULL OR NEW.closure_workspace_id IS NOT NULL
    OR NEW.closure_customer_ref IS NOT NULL OR NEW.closure_subscription_ref IS NOT NULL THEN
    RAISE EXCEPTION 'unknown historical closure context cannot be invented'
      USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END $$;


--
-- Name: clear_stale_actor_user_provenance(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.clear_stale_actor_user_provenance() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  pair_index integer;
  actor_column text;
  user_column text;
  old_row jsonb := to_jsonb(OLD);
  new_row jsonb := to_jsonb(NEW);
BEGIN
  IF TG_NARGS % 2 <> 0 THEN
    RAISE EXCEPTION 'actor provenance trigger requires actor/provenance column pairs';
  END IF;

  FOR pair_index IN 0..((TG_NARGS / 2) - 1) LOOP
    actor_column := TG_ARGV[pair_index * 2];
    user_column := TG_ARGV[pair_index * 2 + 1];
    IF new_row -> actor_column IS DISTINCT FROM old_row -> actor_column
       AND new_row -> user_column IS NOT DISTINCT FROM old_row -> user_column THEN
      NEW := jsonb_populate_record(NEW, jsonb_build_object(user_column, NULL));
      new_row := to_jsonb(NEW);
    END IF;
  END LOOP;
  RETURN NEW;
END $$;


--
-- Name: configure_automation_definition(uuid, text, text, text, integer, text, text, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.configure_automation_definition(target_project_id uuid, target_name text, target_trigger_kind text, target_recipe_id text, target_recipe_version integer, target_status text, target_actor_kind text, target_actor_id text) RETURNS TABLE(id uuid, project_id uuid, name text, trigger_kind text, recipe_id text, recipe_version integer, status text, actor_kind text, actor_id text, created_at timestamp with time zone, updated_at timestamp with time zone)
    LANGUAGE plpgsql
    AS $$
DECLARE
  definition_row_id uuid;
BEGIN
  IF NOT EXISTS (SELECT 1 FROM projects WHERE projects.id = target_project_id) THEN
    RAISE EXCEPTION 'automation project does not exist'
      USING ERRCODE = 'foreign_key_violation';
  END IF;
  IF target_name IS NULL OR length(btrim(target_name)) NOT BETWEEN 1 AND 200 THEN
    RAISE EXCEPTION 'automation name is invalid' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_trigger_kind IS NULL OR target_trigger_kind NOT IN (
    'report_added', 'acquisition_completed', 'full_text_attached',
    'report_included', 'study_created', 'appraisal_completed', 'manual'
  ) THEN
    RAISE EXCEPTION 'automation trigger kind is unknown' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_status IS NULL OR target_status NOT IN ('active', 'paused') THEN
    RAISE EXCEPTION 'automation definition status is unknown' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF target_actor_kind IS NULL OR target_actor_kind NOT IN ('user', 'automation', 'system')
      OR target_actor_id IS NULL OR length(btrim(target_actor_id)) NOT BETWEEN 1 AND 200 THEN
    RAISE EXCEPTION 'automation actor is invalid' USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF NOT EXISTS (
    SELECT 1
    FROM automation_builtin_recipe_steps(target_recipe_id, target_recipe_version)
  ) THEN
    RAISE EXCEPTION 'automation recipe is not a supported built-in recipe'
      USING ERRCODE = 'invalid_parameter_value';
  END IF;

  INSERT INTO automation_definitions
    (project_id, name, trigger_kind, recipe_id, recipe_version, status,
     actor_kind, actor_id)
  VALUES
    (target_project_id, target_name, target_trigger_kind, target_recipe_id,
     target_recipe_version, target_status, target_actor_kind, target_actor_id)
  ON CONFLICT ON CONSTRAINT automation_definitions_project_name_key DO UPDATE SET
    trigger_kind = EXCLUDED.trigger_kind,
    recipe_id = EXCLUDED.recipe_id,
    recipe_version = EXCLUDED.recipe_version,
    status = EXCLUDED.status,
    actor_kind = EXCLUDED.actor_kind,
    actor_id = EXCLUDED.actor_id,
    updated_at = now()
  RETURNING automation_definitions.id INTO definition_row_id;

  DELETE FROM automation_definition_steps
  WHERE automation_definition_steps.project_id = target_project_id
    AND automation_definition_steps.definition_id = definition_row_id;

  INSERT INTO automation_definition_steps
    (project_id, definition_id, ordinal, step_key, step_kind)
  SELECT target_project_id, definition_row_id, steps.ordinal, steps.step_key,
         steps.step_kind
  FROM automation_builtin_recipe_steps(target_recipe_id, target_recipe_version) AS steps;

  RETURN QUERY
  SELECT d.id, d.project_id, d.name, d.trigger_kind, d.recipe_id, d.recipe_version,
         d.status, d.actor_kind, d.actor_id, d.created_at, d.updated_at
  FROM automation_definitions AS d
  WHERE d.project_id = target_project_id AND d.id = definition_row_id;
END;
$$;


--
-- Name: configure_database_roles(text, text, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.configure_database_roles(p_runtime text, p_migration text, p_maintenance text) RETURNS void
    LANGUAGE plpgsql
    SET search_path TO 'pg_catalog'
    AS $_$
DECLARE entry record; capability text; login_name text; first_activation boolean; capabilities text[]:=ARRAY['deepref_runtime','deepref_migration_owner','deepref_maintenance'];
BEGIN
  IF p_runtime IS NULL OR p_migration IS NULL OR p_maintenance IS NULL OR
     p_runtime=p_migration OR p_runtime=p_maintenance OR p_migration=p_maintenance THEN
    RAISE EXCEPTION 'three distinct login identities are required' USING ERRCODE='invalid_parameter_value';
  END IF;
  IF NOT EXISTS(SELECT 1 FROM pg_catalog.pg_database d WHERE d.datname=current_database()
      AND (d.datdba=current_user::regrole OR EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname=current_user AND rolsuper))) THEN
    RAISE EXCEPTION 'database owner administration is required' USING ERRCODE='insufficient_privilege';
  END IF;
  SELECT activated_at IS NULL INTO first_activation
    FROM public.database_role_contract WHERE singleton FOR UPDATE;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'database role contract is missing' USING ERRCODE='integrity_constraint_violation';
  END IF;
  FOREACH capability IN ARRAY capabilities LOOP
    IF NOT EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname=capability) THEN
      EXECUTE format('CREATE ROLE %I NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS',capability);
    END IF;
    IF EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname=capability AND
       (rolcanlogin OR rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls)) OR
       EXISTS(SELECT 1 FROM pg_catalog.pg_auth_members m JOIN pg_catalog.pg_roles r ON r.oid=m.member WHERE r.rolname=capability) THEN
      RAISE EXCEPTION 'capability role has unsafe attributes or memberships' USING ERRCODE='insufficient_privilege';
    END IF;
  END LOOP;
  FOR login_name,capability IN SELECT * FROM unnest(ARRAY[p_runtime,p_migration,p_maintenance],capabilities) LOOP
    IF login_name !~ '^[a-zA-Z_][a-zA-Z0-9_]{0,62}$' OR login_name=ANY(capabilities) OR
       NOT EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname=login_name AND rolcanlogin AND rolinherit AND
         NOT(rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls)) OR
       EXISTS(SELECT 1 FROM pg_catalog.pg_roles r WHERE r.rolname NOT IN(login_name,capability) AND pg_has_role(login_name,r.oid,'MEMBER')) THEN
      RAISE EXCEPTION 'login role is missing, privileged, or has unrelated memberships' USING ERRCODE='insufficient_privilege';
    END IF;
    EXECUTE format('GRANT %I TO %I',capability,login_name);
    EXECUTE format('REVOKE ADMIN OPTION FOR %I FROM %I',capability,login_name);
    EXECUTE format('REVOKE ALL ON ALL TABLES IN SCHEMA public FROM %I',login_name);
    EXECUTE format('REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM %I',login_name);
    EXECUTE format('REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA public FROM %I',login_name);
    EXECUTE format('REVOKE ALL ON SCHEMA public FROM %I',login_name);
    EXECUTE format('REVOKE ALL ON DATABASE %I FROM %I',current_database(),login_name);
  END LOOP;
  EXECUTE format('GRANT CREATE ON DATABASE %I TO deepref_migration_owner',current_database());
  ALTER SCHEMA public OWNER TO deepref_migration_owner;
  -- Adopt application objects, preserving extension ownership. New migrations
  -- run after SET ROLE deepref_migration_owner on every migration connection.
  FOR entry IN SELECT c.oid,c.relname,c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
    WHERE n.nspname='public' AND c.relkind IN('r','p','v','m','S')
      AND NOT EXISTS(SELECT 1 FROM pg_catalog.pg_depend d WHERE d.objid=c.oid AND d.classid='pg_class'::regclass AND d.deptype='e')
    ORDER BY (c.relkind='S'),c.oid LOOP
    EXECUTE format('ALTER %s public.%I OWNER TO deepref_migration_owner',CASE WHEN entry.relkind='S' THEN 'SEQUENCE' ELSE 'TABLE' END,entry.relname);
  END LOOP;
  FOR entry IN SELECT p.oid FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace
    WHERE n.nspname='public' AND p.prokind IN('f','w')
      AND NOT EXISTS(SELECT 1 FROM pg_catalog.pg_depend d WHERE d.objid=p.oid AND d.classid='pg_proc'::regclass AND d.deptype='e') LOOP
    EXECUTE format('ALTER FUNCTION %s OWNER TO deepref_migration_owner',entry.oid::regprocedure);
  END LOOP;
  ALTER SCHEMA public OWNER TO deepref_migration_owner;
  REVOKE ALL ON SCHEMA public FROM PUBLIC;
  GRANT USAGE ON SCHEMA public TO deepref_runtime,deepref_maintenance;
  EXECUTE format('REVOKE ALL ON DATABASE %I FROM PUBLIC',current_database());
  EXECUTE format('GRANT CONNECT ON DATABASE %I TO deepref_runtime,deepref_migration_owner,deepref_maintenance',current_database());
  EXECUTE format('GRANT CREATE ON DATABASE %I TO deepref_migration_owner',current_database());
  REVOKE ALL ON ALL TABLES IN SCHEMA public FROM PUBLIC;
  -- Initial adoption supplies the existing ordinary application grants. Later
  -- activation must preserve narrower grants declared by subsequent migrations.
  IF first_activation THEN
    REVOKE ALL ON ALL TABLES IN SCHEMA public FROM deepref_runtime,deepref_maintenance;
    GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA public TO deepref_runtime;
  END IF;
  -- Relation-level REVOKE does not remove column ACLs. Clear direct LOGIN and
  -- PUBLIC column grants explicitly; capability grants are adopted once, then
  -- preserved except for the fixed protected families below.
  FOR entry IN
    SELECT c.relname,string_agg(format('%I',a.attname),',' ORDER BY a.attnum) AS column_names
    FROM pg_catalog.pg_class c
    JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
    JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
    WHERE n.nspname='public' AND c.relkind IN('r','p','v','m')
    GROUP BY c.oid,c.relname
  LOOP
    EXECUTE format('REVOKE SELECT (%1$s), INSERT (%1$s), UPDATE (%1$s), REFERENCES (%1$s) ON TABLE public.%2$I FROM PUBLIC, %3$I, %4$I, %5$I',
      entry.column_names,entry.relname,p_runtime,p_migration,p_maintenance);
    IF first_activation THEN
      EXECUTE format('REVOKE SELECT (%1$s), INSERT (%1$s), UPDATE (%1$s), REFERENCES (%1$s) ON TABLE public.%2$I FROM deepref_runtime,deepref_maintenance',
        entry.column_names,entry.relname);
    END IF;
    EXECUTE format('REVOKE REFERENCES (%s) ON TABLE public.%I FROM deepref_runtime,deepref_maintenance',entry.column_names,entry.relname);
    IF entry.relname IN('admin_audit_events','fulfilled_account_deletions') THEN
      EXECUTE format('REVOKE UPDATE (%s) ON TABLE public.%I FROM deepref_runtime',entry.column_names,entry.relname);
    END IF;
    IF entry.relname IN('_sqlx_migrations','database_role_contract','account_export_rollout','account_erasure_rollout') THEN
      EXECUTE format('REVOKE INSERT (%1$s), UPDATE (%1$s) ON TABLE public.%2$I FROM deepref_runtime',entry.column_names,entry.relname);
    END IF;
    IF entry.relname IN('admin_audit_events','fulfilled_account_deletions','user_sessions','database_role_contract','_sqlx_migrations','account_export_rollout','account_erasure_rollout') THEN
      EXECUTE format('REVOKE SELECT (%1$s), INSERT (%1$s), UPDATE (%1$s) ON TABLE public.%2$I FROM deepref_maintenance',entry.column_names,entry.relname);
    END IF;
  END LOOP;
  REVOKE TRUNCATE,REFERENCES,TRIGGER ON ALL TABLES IN SCHEMA public FROM deepref_runtime,deepref_maintenance;
  REVOKE ALL ON public._sqlx_migrations,public.database_role_contract FROM deepref_runtime;
  GRANT SELECT ON public._sqlx_migrations,public.database_role_contract TO deepref_runtime;
  -- Both operator rollouts remain readable by runtime business flows; only
  -- exact maintenance activation functions can mutate their control rows.
  REVOKE ALL ON public.account_export_rollout,public.account_erasure_rollout FROM deepref_runtime,deepref_maintenance;
  GRANT SELECT ON public.account_export_rollout,public.account_erasure_rollout TO deepref_runtime;
  REVOKE ALL ON FUNCTION public.activate_export_retention(text),public.activate_account_erasure(text) FROM deepref_runtime;

  REVOKE UPDATE,DELETE ON public.admin_audit_events,public.fulfilled_account_deletions FROM deepref_runtime;
  REVOKE ALL ON public.admin_audit_events,public.fulfilled_account_deletions,public.user_sessions,public.database_role_contract,public._sqlx_migrations FROM deepref_maintenance;
  GRANT SELECT (event_type,occurred_at) ON public.admin_audit_events TO deepref_maintenance;
  GRANT SELECT (completed_at) ON public.fulfilled_account_deletions TO deepref_maintenance;
  GRANT SELECT (revoked_at,expires_at) ON public.user_sessions TO deepref_maintenance;
  GRANT SELECT ON public.database_role_contract TO deepref_maintenance;
  -- Sequence ACLs are separate from table ACLs. Repair drift every time,
  -- without widening a runtime sequence narrowed by a later migration.
  REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM PUBLIC,deepref_maintenance;
  REVOKE UPDATE ON ALL SEQUENCES IN SCHEMA public FROM deepref_runtime;
  IF first_activation THEN
    REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM deepref_runtime;
    GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA public TO deepref_runtime;
  END IF;
  REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA public FROM PUBLIC;
  IF first_activation THEN
    REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA public FROM deepref_runtime,deepref_maintenance;
    FOR entry IN SELECT p.oid FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace
    WHERE n.nspname='public' AND NOT p.prosecdef AND p.prokind IN('f','w')
      AND p.proname<>'configure_database_roles' LOOP
      EXECUTE format('GRANT EXECUTE ON FUNCTION %s TO deepref_runtime',entry.oid::regprocedure);
    END LOOP;
  END IF;
  GRANT EXECUTE ON FUNCTION public.redact_deleted_user_audit(uuid) TO deepref_runtime;
  GRANT EXECUTE ON FUNCTION public.purge_retention_records(integer) TO deepref_maintenance;
  GRANT EXECUTE ON FUNCTION public.activate_export_retention(text),public.activate_account_erasure(text) TO deepref_maintenance;
  ALTER DEFAULT PRIVILEGES FOR ROLE deepref_migration_owner IN SCHEMA public GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO deepref_runtime;
  -- Remove unsafe global defaults too: a schema-local REVOKE cannot cancel a
  -- global default grant when the next migration creates an application sequence.
  ALTER DEFAULT PRIVILEGES FOR ROLE deepref_migration_owner REVOKE ALL ON SEQUENCES FROM PUBLIC,deepref_maintenance;
  ALTER DEFAULT PRIVILEGES FOR ROLE deepref_migration_owner IN SCHEMA public REVOKE ALL ON SEQUENCES FROM PUBLIC,deepref_maintenance;
  ALTER DEFAULT PRIVILEGES FOR ROLE deepref_migration_owner REVOKE UPDATE ON SEQUENCES FROM deepref_runtime;
  ALTER DEFAULT PRIVILEGES FOR ROLE deepref_migration_owner IN SCHEMA public REVOKE UPDATE ON SEQUENCES FROM deepref_runtime;
  IF first_activation THEN
    ALTER DEFAULT PRIVILEGES FOR ROLE deepref_migration_owner IN SCHEMA public GRANT USAGE,SELECT ON SEQUENCES TO deepref_runtime;
  END IF;
  ALTER DEFAULT PRIVILEGES FOR ROLE deepref_migration_owner REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;
  UPDATE public.database_role_contract SET activated_at=clock_timestamp(),activated_by=session_user WHERE singleton;
END;
$_$;


--
-- Name: deepref_seed_full_text_reasons(uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.deepref_seed_full_text_reasons(project uuid) RETURNS void
    LANGUAGE sql
    AS $$
  UPDATE exclusion_reasons
  SET code = 'wrong_comparator_outcome', label = 'Wrong comparator/outcome', stage = 'full_text'
  WHERE project_id = project AND code = 'wrong_outcome';
  UPDATE exclusion_reasons
  SET code = 'insufficient_information', label = 'Insufficient information', stage = 'full_text'
  WHERE project_id = project AND code = 'no_usable_full_text';
  INSERT INTO exclusion_reasons(id, project_id, code, label, stage)
  SELECT gen_random_uuid(), project, reason.code, reason.label, 'full_text'
  FROM (VALUES
    ('wrong_population', 'Wrong population'),
    ('wrong_intervention', 'Wrong intervention or exposure'),
    ('wrong_comparator_outcome', 'Wrong comparator/outcome'),
    ('wrong_design', 'Wrong study design'),
    ('conference_abstract_only', 'Conference abstract only'),
    ('duplicate_overlapping_dataset', 'Duplicate or overlapping dataset'),
    ('insufficient_information', 'Insufficient information'),
    ('other', 'Other')
  ) AS reason(code, label)
  ON CONFLICT(project_id, code) DO UPDATE SET label = EXCLUDED.label, stage = 'full_text';
$$;


--
-- Name: deepref_seed_full_text_reasons_on_project(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.deepref_seed_full_text_reasons_on_project() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  PERFORM deepref_seed_full_text_reasons(NEW.id);
  RETURN NEW;
END $$;


--
-- Name: deepref_touch_exclusion_reason_updated_at(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.deepref_touch_exclusion_reason_updated_at() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  NEW.updated_at = now();
  RETURN NEW;
END;
$$;


--
-- Name: enforce_account_physical_proof(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.enforce_account_physical_proof() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'pg_catalog', 'public'
    AS $$
BEGIN
  IF TG_OP='UPDATE' THEN
    IF NEW.physical_verified_at IS DISTINCT FROM OLD.physical_verified_at
       OR NEW.logical_scrubbed_at IS DISTINCT FROM OLD.logical_scrubbed_at THEN
      RAISE EXCEPTION 'physical verification cannot be retroactively assigned';
    END IF;
    RETURN NEW;
  END IF;
  IF NEW.physical_verified_at IS NULL THEN
    IF EXISTS(SELECT 1 FROM public.account_erasure_rollout WHERE singleton AND activated) THEN
      RAISE EXCEPTION 'new completions require physical deletion acknowledgements';
    END IF;
    RETURN NEW;
  END IF;
  IF NEW.logical_scrubbed_at IS DISTINCT FROM (SELECT logical_scrubbed_at FROM public.account_erasure_progress WHERE user_id=NEW.user_id) THEN
    RAISE EXCEPTION 'physical proof must preserve its original logical scrub cutoff';
  END IF;
  IF NOT EXISTS(SELECT 1 FROM public.account_erasure_rollout WHERE singleton AND activated)
     OR NOT EXISTS(SELECT 1 FROM public.account_erasure_progress p
      WHERE p.user_id=NEW.user_id AND p.due_at<=clock_timestamp()
        AND p.logical_scrubbed_at IS NOT NULL)
     OR EXISTS(SELECT 1 FROM public.account_erasure_objects t
       JOIN public.object_lifecycle o USING(storage_key)
       WHERE t.user_id=NEW.user_id AND o.deleted_at IS NULL)
     OR EXISTS(SELECT 1 FROM public.account_erasure_writers t
       JOIN public.account_export_writers w USING(attempt_id)
       WHERE t.user_id=NEW.user_id AND (w.stopped_at IS NULL OR w.upload_outcome IN ('dispatched','unresolved')))
  THEN
    RAISE EXCEPTION 'physical deletion acknowledgements are incomplete';
  END IF;
  NEW.physical_verified_at:=clock_timestamp();
  RETURN NEW;
END $$;


--
-- Name: enforce_ai_run_parent_automation_project(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.enforce_ai_run_parent_automation_project() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.parent_automation_run_id IS NOT NULL AND NOT EXISTS(
   SELECT 1 FROM durable_runs r WHERE r.id=NEW.parent_automation_run_id
     AND r.project_id=NEW.project_id AND r.kind='automation') THEN
  RAISE EXCEPTION 'AI run parent automation run belongs to another project' USING ERRCODE='foreign_key_violation';
 END IF;
 RETURN NEW;
END $$;


--
-- Name: enforce_anonymized_deleted_user(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.enforce_anonymized_deleted_user() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF OLD.deleted_at IS NOT NULL THEN
    NEW.deleted_at := OLD.deleted_at;
    NEW.status := 'deactivated';
    NEW.primary_email := 'Former member';
    NEW.display_name := 'Former member';
    NEW.verified_email := NULL;
    NEW.workos_user_id := 'deleted:' || OLD.id;
  END IF;
  RETURN NEW;
END;
$$;


--
-- Name: enforce_object_retention_target(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.enforce_object_retention_target() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'pg_catalog', 'public'
    AS $$
DECLARE live boolean;
BEGIN
  SELECT retired_at IS NULL AND deleted_at IS NULL
      AND (privacy_due_at IS NULL OR privacy_due_at>clock_timestamp())
    INTO live FROM public.object_lifecycle WHERE storage_key=NEW.storage_key FOR UPDATE;
  IF NOT coalesce(live,false) THEN
    RAISE EXCEPTION 'retention target is retired or due for privacy erasure';
  END IF;
  RETURN NEW;
END $$;


--
-- Name: guard_ai_first_audit_label_identity(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.guard_ai_first_audit_label_identity() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  PERFORM pg_advisory_xact_lock_shared(73003203);
  IF EXISTS (
    SELECT 1 FROM ai_screening_audit_labels l
    WHERE l.cohort_id = NEW.cohort_id AND l.report_id = NEW.report_id
      AND l.id IS DISTINCT FROM CASE WHEN TG_OP = 'UPDATE' THEN OLD.id ELSE NULL END
      AND same_human_actor_identity(
        'user', NEW.actor_id, NEW.actor_user_id,
        'user', l.actor_id, l.actor_user_id
      )
  ) THEN
    RAISE EXCEPTION 'audit label is not provably distinct from an existing reviewer'
      USING ERRCODE = 'check_violation';
  END IF;
  RETURN NEW;
END $$;


--
-- Name: guard_ai_first_identity_transition(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.guard_ai_first_identity_transition() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.status IN ('passed', 'finalized') AND NEW.status IS DISTINCT FROM OLD.status THEN
    PERFORM pg_advisory_xact_lock_shared(73003203);
    IF EXISTS (
      SELECT 1 FROM ai_screening_cohort_members m
      WHERE m.cohort_id = NEW.id AND (m.sampled OR m.audit_control)
        AND NOT ai_first_audit_has_two_distinct_reviewers(m.cohort_id, m.report_id)
    ) THEN
      RAISE EXCEPTION 'two resolved, distinct reviewers are required for every audit record'
        USING ERRCODE = 'check_violation';
    END IF;
  END IF;
  RETURN NEW;
END $$;


--
-- Name: ingestions_preserve_historical_depth(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.ingestions_preserve_historical_depth() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.max_depth > 3 AND NEW.historical_max_depth IS NULL THEN
        NEW.historical_max_depth := NEW.max_depth;
    END IF;
    RETURN NEW;
END;
$$;


--
-- Name: invalidate_inactivity_contact(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.invalidate_inactivity_contact() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'pg_catalog', 'public'
    AS $$
BEGIN
  UPDATE public.account_human_activity SET contact_generation=gen_random_uuid()
    WHERE user_id=NEW.id;
  RETURN NEW;
END $$;


--
-- Name: job_wakeup_event_id(uuid, bigint); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.job_wakeup_event_id(job_id uuid, generation bigint) RETURNS uuid
    LANGUAGE sql IMMUTABLE STRICT
    AS $$
  SELECT md5('deepref.work:' || job_id::text || ':' || generation::text)::uuid
$$;


--
-- Name: jobs_fill_workload_class(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.jobs_fill_workload_class() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.workload_class IS NULL THEN
    NEW.workload_class := jobs_workload_class_for_kind(NEW.kind);
  END IF;
  RETURN NEW;
END;
$$;


--
-- Name: jobs_increment_claim_seq(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.jobs_increment_claim_seq() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  NEW.claim_seq := OLD.claim_seq + 1;
  RETURN NEW;
END;
$$;


--
-- Name: jobs_prepare_wakeup(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.jobs_prepare_wakeup() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP = 'UPDATE' THEN
    IF NEW.state IN ('queued', 'capability_wait') AND
       (OLD.state NOT IN ('queued', 'capability_wait') OR NEW.available_at IS DISTINCT FROM OLD.available_at) THEN
      NEW.wakeup_generation := OLD.wakeup_generation + 1;
    END IF;
  END IF;
  RETURN NEW;
END $$;


--
-- Name: jobs_publish_wakeup(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.jobs_publish_wakeup() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.state IN ('queued', 'capability_wait') AND NEW.available_at <= now()
     AND NEW.attempts < NEW.max_attempts
     AND NEW.wakeup_emitted_generation < NEW.wakeup_generation THEN
    INSERT INTO outbox_events (event_id, message_type, message_version, payload, expected_consumers)
    VALUES (job_wakeup_event_id(NEW.id, NEW.wakeup_generation), 'deepref.work', 1,
      jsonb_build_object('job_id', NEW.id, 'generation', NEW.wakeup_generation,
        'workload_class', coalesce(NEW.workload_class, jobs_workload_class_for_kind(NEW.kind))),
      ARRAY['durable-work']) ON CONFLICT (event_id) DO UPDATE
      SET published_at=NULL, publication_epoch=outbox_events.publication_epoch+1,
          publication_next_at=now(), publication_owner=NULL, publication_until=NULL
      WHERE outbox_events.delivery_state='pending';
    -- The nested update cannot publish again: its NEW marker equals generation.
    UPDATE jobs SET wakeup_emitted_generation=NEW.wakeup_generation
      WHERE id=NEW.id AND wakeup_generation=NEW.wakeup_generation
        AND wakeup_emitted_generation<NEW.wakeup_generation;
  END IF;
  RETURN NEW;
END $$;


--
-- Name: jobs_settle_wakeup(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.jobs_settle_wakeup() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE event uuid;
BEGIN
  IF (OLD.state = 'running' AND NEW.state <> 'running')
     OR NEW.wakeup_generation <> OLD.wakeup_generation
     OR NEW.state IN ('completed', 'failed', 'dead') THEN
    event := job_wakeup_event_id(OLD.id, OLD.wakeup_generation);
    INSERT INTO outbox_receipts (event_id, consumer, state, delivered_at)
      SELECT event, 'durable-work', 'delivered', now()
      WHERE EXISTS (SELECT 1 FROM outbox_events WHERE event_id = event)
      ON CONFLICT (event_id, consumer) DO UPDATE SET state='delivered', delivered_at=now(), last_error=NULL;
    UPDATE outbox_events SET delivery_state='delivered', delivered_at=now(),
      lease_owner=NULL, lease_expires_at=NULL
      WHERE event_id=event;
  END IF;
  RETURN NEW;
END $$;


--
-- Name: jobs_workload_class_for_kind(text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.jobs_workload_class_for_kind(kind text) RETURNS text
    LANGUAGE sql IMMUTABLE
    AS $_$
  SELECT CASE $1
    WHEN 'assistant_agent_run' THEN 'ai'
    WHEN 'ai_usage_reconcile' THEN 'ai'
    WHEN 'ai_usage_export_replay' THEN 'ai'
    WHEN 'retrieve_document' THEN 'documents'
    WHEN 'parse_document' THEN 'documents'
    WHEN 'run_deduplication' THEN 'documents'
    WHEN 'cleanup_objects' THEN 'documents'
    WHEN 'account_data_export' THEN 'documents'
    WHEN 'durable_run_step' THEN 'workflow'
    WHEN 'backfill_workflow_files' THEN 'workflow'
    WHEN 'work_fetch_requested' THEN 'integration'
    WHEN 'pmid_import' THEN 'integration'
    WHEN 'recompute_metrics' THEN 'projection'
    WHEN 'recompute_prisma' THEN 'projection'
    ELSE 'workflow'
  END
$_$;


--
-- Name: legacy_human_actor_is_unmapped(text, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.legacy_human_actor_is_unmapped(p_actor_kind text, p_actor_id text) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
  SELECT p_actor_kind = 'user'
    AND p_actor_id IS NOT NULL
    AND NOT EXISTS (
      SELECT 1 FROM legacy_actor_mappings m
      WHERE m.legacy_actor_kind = p_actor_kind
        AND m.legacy_actor_id = p_actor_id
    )
$$;


--
-- Name: legacy_human_actor_matches_user(text, text, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.legacy_human_actor_matches_user(p_actor_kind text, p_actor_id text, p_user_id uuid) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
  SELECT p_actor_kind = 'user'
    AND EXISTS (
      SELECT 1 FROM legacy_actor_mappings m
      WHERE m.legacy_actor_kind = p_actor_kind
        AND m.legacy_actor_id = p_actor_id
        AND m.user_id = p_user_id
    )
$$;


--
-- Name: legacy_human_actor_user_id(text, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.legacy_human_actor_user_id(p_actor_kind text, p_actor_id text) RETURNS uuid
    LANGUAGE sql STABLE
    AS $$
  SELECT m.user_id
  FROM legacy_actor_mappings m
  WHERE p_actor_kind = 'user'
    AND m.legacy_actor_kind = p_actor_kind
    AND m.legacy_actor_id = p_actor_id
$$;


--
-- Name: list_automation_definitions(uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.list_automation_definitions(target_project_id uuid) RETURNS TABLE(id uuid, project_id uuid, name text, trigger_kind text, recipe_id text, recipe_version integer, status text, actor_kind text, actor_id text, created_at timestamp with time zone, updated_at timestamp with time zone, steps jsonb)
    LANGUAGE sql STABLE
    AS $$
  SELECT d.id, d.project_id, d.name, d.trigger_kind, d.recipe_id, d.recipe_version,
         d.status, d.actor_kind, d.actor_id, d.created_at, d.updated_at,
         COALESCE(
           jsonb_agg(
             jsonb_build_object(
               'ordinal', s.ordinal,
               'key', s.step_key,
               'kind', s.step_kind
             ) ORDER BY s.ordinal
           ) FILTER (WHERE s.ordinal IS NOT NULL),
           '[]'::jsonb
         ) AS steps
  FROM automation_definitions AS d
  LEFT JOIN automation_definition_steps AS s
    ON s.project_id = d.project_id AND s.definition_id = d.id
  WHERE d.project_id = target_project_id
  GROUP BY d.id
  ORDER BY d.name, d.id
$$;


--
-- Name: object_cleanup_jobs_fence(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.object_cleanup_jobs_fence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.kind <> 'cleanup_objects' THEN RETURN NEW; END IF;
  IF NOT coalesce(NEW.kind = ANY(string_to_array(current_setting('deepref.job_kinds', true), ',')), false) THEN
    IF NEW.state = 'running' AND (TG_OP = 'INSERT' OR OLD.state <> 'running') THEN RETURN NULL; END IF;
    IF TG_OP = 'UPDATE' AND OLD.state = 'running' AND OLD.leased_until < now() THEN RETURN NULL; END IF;
  END IF;
  IF NEW.state = 'queued' THEN NEW.state := 'capability_wait'; END IF;
  RETURN NEW;
END;
$$;


--
-- Name: object_reference_changed(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.object_reference_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  previous_key text;
  next_key text;
  tenant uuid;
  owner_project uuid;
  entry object_lifecycle;
BEGIN
  IF TG_TABLE_NAME = 'documents' THEN
    IF TG_OP <> 'INSERT' THEN previous_key := OLD.object_key; END IF;
    IF TG_OP <> 'DELETE' THEN next_key := NEW.object_key; END IF;
  ELSE
    IF TG_OP <> 'INSERT' THEN previous_key := OLD.storage_key; END IF;
    IF TG_OP <> 'DELETE' THEN next_key := NEW.storage_key; END IF;
  END IF;
  IF previous_key IS NOT DISTINCT FROM next_key THEN RETURN NULL; END IF;
  -- Lock keys in sorted order even on replacement, avoiding inverted key locks.
  PERFORM 1 FROM object_lifecycle WHERE storage_key IN (previous_key, next_key)
    ORDER BY storage_key FOR UPDATE;
  IF next_key IS NOT NULL THEN
    owner_project := NEW.project_id;
    SELECT workspace_id INTO STRICT tenant FROM projects WHERE id=owner_project;
    IF next_key LIKE 'projects/%' AND next_key NOT LIKE 'projects/' || owner_project::text || '/workflow-runs/%' THEN
      RAISE EXCEPTION 'object reference belongs to another project';
    END IF;
    IF next_key LIKE 'workspaces/%' AND
       next_key NOT LIKE 'workspaces/' || tenant::text || '/projects/' || owner_project::text || '/%' THEN
      RAISE EXCEPTION 'object reference belongs to another tenant/project';
    END IF;
    INSERT INTO object_lifecycle(storage_key, workspace_id, project_id)
      VALUES(next_key, tenant, owner_project) ON CONFLICT DO NOTHING;
    SELECT * INTO STRICT entry FROM object_lifecycle WHERE storage_key=next_key FOR UPDATE;
    IF entry.retired_at IS NOT NULL OR entry.deleted_at IS NOT NULL OR entry.privacy_due_at<=clock_timestamp() THEN
      RAISE EXCEPTION 'object reference has been retired';
    END IF;
    IF entry.workspace_id IS DISTINCT FROM tenant THEN
      RAISE EXCEPTION 'object reference belongs to another tenant';
    END IF;
    UPDATE object_lifecycle SET cleanup_after=NULL WHERE storage_key=next_key;
  END IF;
  IF previous_key IS NOT NULL THEN
    UPDATE object_lifecycle SET cleanup_after=CASE WHEN privacy_due_at<=clock_timestamp() THEN clock_timestamp() ELSE now()+interval '24 hours' END
      WHERE storage_key=previous_key AND deleted_at IS NULL;
  END IF;
  RETURN NULL;
END;
$$;


--
-- Name: prevent_published_criterion_mutation(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.prevent_published_criterion_mutation() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
  protocol_id uuid;
BEGIN
  -- Criteria are deleted by the protocol/project FK during aggregate cleanup.
  IF TG_OP = 'DELETE' AND pg_trigger_depth() > 1 THEN
    RETURN OLD;
  END IF;

  IF TG_OP = 'DELETE' THEN
    protocol_id := OLD.protocol_version_id;
  ELSE
    protocol_id := NEW.protocol_version_id;
  END IF;

  IF EXISTS (
    SELECT 1
    FROM protocol_versions
    WHERE id = protocol_id AND status IN ('published', 'superseded')
  ) THEN
    RAISE EXCEPTION 'published protocol criteria are immutable'
      USING ERRCODE = 'check_violation';
  END IF;

  IF TG_OP = 'UPDATE' AND OLD.protocol_version_id IS DISTINCT FROM NEW.protocol_version_id
     AND EXISTS (
       SELECT 1
       FROM protocol_versions
       WHERE id = OLD.protocol_version_id AND status IN ('published', 'superseded')
     ) THEN
    RAISE EXCEPTION 'published protocol criteria are immutable'
      USING ERRCODE = 'check_violation';
  END IF;

  IF TG_OP = 'DELETE' THEN
    RETURN OLD;
  END IF;
  RETURN NEW;
END;
$$;


--
-- Name: prevent_published_protocol_content_mutation(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.prevent_published_protocol_content_mutation() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP = 'DELETE' THEN
    -- Cascading project deletion invokes this trigger at a deeper level. The
    -- project FK owns that cleanup; direct deletion of a historical artifact
    -- remains forbidden.
    IF pg_trigger_depth() > 1 THEN
      RETURN OLD;
    END IF;
    IF OLD.status IN ('published', 'superseded') THEN
      RAISE EXCEPTION 'published protocol scientific content is immutable'
        USING ERRCODE = 'check_violation';
    END IF;
    RETURN OLD;
  END IF;

  IF OLD.status = 'superseded' AND NEW.status <> 'superseded' THEN
    RAISE EXCEPTION 'superseded protocol lifecycle is immutable'
      USING ERRCODE = 'check_violation';
  END IF;
  IF OLD.status = 'published' AND NEW.status NOT IN ('published', 'superseded') THEN
    RAISE EXCEPTION 'published protocol can only become superseded'
      USING ERRCODE = 'check_violation';
  END IF;
  IF OLD.status IN ('published', 'superseded') AND (
    OLD.project_id IS DISTINCT FROM NEW.project_id OR
    OLD.version IS DISTINCT FROM NEW.version OR
    OLD.name IS DISTINCT FROM NEW.name OR
    OLD.criteria IS DISTINCT FROM NEW.criteria OR
    OLD.framework_kind IS DISTINCT FROM NEW.framework_kind OR
    OLD.framework_fields IS DISTINCT FROM NEW.framework_fields OR
    OLD.objective IS DISTINCT FROM NEW.objective OR
    OLD.question IS DISTINCT FROM NEW.question OR
    OLD.amendment_of IS DISTINCT FROM NEW.amendment_of OR
    OLD.revision IS DISTINCT FROM NEW.revision
  ) THEN
    RAISE EXCEPTION 'published protocol scientific content is immutable'
      USING ERRCODE = 'check_violation';
  END IF;
  RETURN NEW;
END;
$$;


--
-- Name: protect_ai_first_evidence(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.protect_ai_first_evidence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF TG_TABLE_NAME='ai_screening_audit_labels' THEN
   RAISE EXCEPTION 'audit labels are immutable';
 ELSIF TG_TABLE_NAME='ai_screening_dispositions' THEN
   IF (to_jsonb(NEW)-ARRAY['voided_at','void_reason','finalized_at','finalized_event_id'])
      IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['voided_at','void_reason','finalized_at','finalized_event_id'])
      OR OLD.voided_at IS NOT NULL OR OLD.finalized_at IS NOT NULL THEN
     RAISE EXCEPTION 'disposition evidence is immutable';
   END IF;
 ELSIF OLD.sampling_seed IS NOT NULL AND
   (to_jsonb(NEW)-ARRAY['status','result','evaluated_at','final_approval_id','finalized_by',
                        'finalized_by_actor_user_id','finalized_at','invalidation_reason',
                        'identity_flagged_at','identity_flag_reason'])
   IS DISTINCT FROM
   (to_jsonb(OLD)-ARRAY['status','result','evaluated_at','final_approval_id','finalized_by',
                        'finalized_by_actor_user_id','finalized_at','invalidation_reason',
                        'identity_flagged_at','identity_flag_reason']) THEN
   RAISE EXCEPTION 'sample design is frozen';
 END IF;
 RETURN NEW;
END $$;


--
-- Name: protect_ai_first_frame(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.protect_ai_first_frame() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM ai_screening_cohorts WHERE id=OLD.cohort_id AND sampling_seed IS NOT NULL)
    AND NEW IS DISTINCT FROM OLD THEN RAISE EXCEPTION 'audit frame is frozen'; END IF;
 RETURN NEW;
END $$;


--
-- Name: protect_durable_identity(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.protect_durable_identity() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE mutable text[]; BEGIN
 mutable:=CASE TG_TABLE_NAME
  WHEN 'durable_runs' THEN ARRAY['status','error','started_at','finished_at']
  WHEN 'durable_steps' THEN ARRAY['status','attempts','claimed_by','claimed_job_id','claimed_job_attempt','started_at','finished_at','error','output']
  WHEN 'durable_attempts' THEN ARRAY['status','finished_at','error']
  WHEN 'automation_step_details' THEN ARRAY['accepted_attempt_id','input_fingerprint']
  WHEN 'workflow_step_details' THEN ARRAY['input','fired_ports','items','note','wait_state']
  ELSE ARRAY[]::text[] END;
 IF (to_jsonb(NEW)-mutable) IS DISTINCT FROM (to_jsonb(OLD)-mutable) THEN
  RAISE EXCEPTION 'durable identity and domain snapshot are immutable' USING ERRCODE='integrity_constraint_violation';
 END IF;
 IF TG_TABLE_NAME='durable_attempts' THEN
  IF OLD.status<>'running' AND NEW IS DISTINCT FROM OLD THEN
   RAISE EXCEPTION 'terminal durable attempt is immutable' USING ERRCODE='integrity_constraint_violation';
  END IF;
 END IF;
 IF TG_TABLE_NAME='automation_step_details' THEN
  IF OLD.accepted_attempt_id IS NOT NULL AND NEW IS DISTINCT FROM OLD THEN
   RAISE EXCEPTION 'accepted recipe evidence is immutable' USING ERRCODE='integrity_constraint_violation';
  END IF;
 END IF;
 RETURN NEW;
END $$;


--
-- Name: protect_review_attempt_detail(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.protect_review_attempt_detail() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF OLD.accepted_at IS NOT NULL AND NEW IS DISTINCT FROM OLD THEN
  RAISE EXCEPTION 'accepted review attempt is immutable' USING ERRCODE='integrity_constraint_violation';
 END IF;
 IF NEW.id IS DISTINCT FROM OLD.id OR NEW.project_id IS DISTINCT FROM OLD.project_id
   OR NEW.run_id IS DISTINCT FROM OLD.run_id OR NEW.node_id IS DISTINCT FROM OLD.node_id
   OR NEW.node_version IS DISTINCT FROM OLD.node_version
   OR NEW.input_fingerprint IS DISTINCT FROM OLD.input_fingerprint THEN
  RAISE EXCEPTION 'review attempt identity is immutable' USING ERRCODE='integrity_constraint_violation';
 END IF;
 RETURN NEW;
END $$;


--
-- Name: purge_retention_records(integer); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.purge_retention_records(p_batch integer) RETURNS TABLE(batches integer, audit_rows bigint, fulfilled_rows bigint, session_rows bigint)
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'pg_catalog'
    SET lock_timeout TO '250ms'
    AS $$
DECLARE changed bigint;
BEGIN
  IF p_batch<1 OR p_batch>1000 OR p_batch IS NULL THEN
    RAISE EXCEPTION 'retention batch must be 1..1000' USING ERRCODE='invalid_parameter_value';
  END IF;
  audit_rows:=0; batches:=0;
  PERFORM pg_catalog.set_config('deepref.audit_purge','on',true);
  FOR i IN 1..32 LOOP
    DELETE FROM public.admin_audit_events WHERE id IN (
      SELECT id FROM public.admin_audit_events
      WHERE occurred_at < now()-CASE WHEN event_type='account.deletion_completed' THEN interval '5 years' ELSE interval '6 months' END
      ORDER BY occurred_at,id LIMIT p_batch FOR UPDATE SKIP LOCKED);
    GET DIAGNOSTICS changed=ROW_COUNT;
    audit_rows:=audit_rows+changed; batches:=batches+1;
    EXIT WHEN changed<p_batch;
  END LOOP;
  DELETE FROM public.fulfilled_account_deletions WHERE user_id IN (
    SELECT user_id FROM public.fulfilled_account_deletions WHERE completed_at<now()-interval '5 years'
    ORDER BY completed_at,user_id LIMIT p_batch FOR UPDATE SKIP LOCKED);
  GET DIAGNOSTICS fulfilled_rows=ROW_COUNT;
  DELETE FROM public.user_sessions WHERE id IN (
    SELECT id FROM public.user_sessions
    WHERE revoked_at<now()-interval '30 days' OR expires_at<now()-interval '30 days'
    ORDER BY coalesce(revoked_at,expires_at),id LIMIT p_batch FOR UPDATE SKIP LOCKED);
  GET DIAGNOSTICS session_rows=ROW_COUNT;
  INSERT INTO public.admin_audit_events(id,event_type,principal_kind,metadata)
    VALUES(pg_catalog.gen_random_uuid(),'audit.purged','system',pg_catalog.jsonb_build_object('rows_purged',audit_rows));
  RETURN NEXT;
END;
$$;


--
-- Name: recompute_project_report_metrics(uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.recompute_project_report_metrics(target_project_id uuid) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE
  max_total double precision;
  max_internal double precision;
  max_outbound double precision;
  current_revision bigint;
  computed_at timestamptz := now();
  report_count bigint;
  edge_count bigint;
BEGIN
  PERFORM 1 FROM projects WHERE id=target_project_id FOR UPDATE;
  UPDATE project_reports pr
  SET total_citations = COALESCE(r.total_citations, 0),
      references_count = COALESCE(r.references_count, 0),
      internal_citations = COALESCE(inbound.internal_count, 0),
      outbound_internal_references = COALESCE(outbound.outbound_count, 0),
      metrics_computed_at = computed_at
  FROM reports r
  LEFT JOIN (
    SELECT c.target_report_id AS report_id,
           count(DISTINCT c.source_report_id)::bigint AS internal_count
    FROM citations c
    JOIN project_reports source_membership
      ON source_membership.project_id = c.project_id
     AND source_membership.report_id = c.source_report_id
    WHERE c.project_id = target_project_id
    GROUP BY c.target_report_id
  ) inbound ON inbound.report_id = r.id
  LEFT JOIN (
    SELECT c.source_report_id AS report_id,
           count(DISTINCT c.target_report_id)::bigint AS outbound_count
    FROM citations c
    JOIN project_reports target_membership
      ON target_membership.project_id = c.project_id
     AND target_membership.report_id = c.target_report_id
    WHERE c.project_id = target_project_id
    GROUP BY c.source_report_id
  ) outbound ON outbound.report_id = r.id
  WHERE pr.project_id = target_project_id AND pr.report_id = r.id;

  SELECT
    GREATEST(COALESCE(MAX(LOG(GREATEST(pr.total_citations, 0) + 1)), 1), 1),
    GREATEST(COALESCE(MAX(pr.internal_citations), 0), 1),
    GREATEST(COALESCE(MAX(pr.outbound_internal_references), 0), 1)
  INTO max_total, max_internal, max_outbound
  FROM project_reports pr
  WHERE pr.project_id = target_project_id;

  UPDATE project_reports pr
  SET rank_score =
        0.45 * (LOG(GREATEST(pr.total_citations, 0) + 1) / max_total) +
        0.40 * (pr.internal_citations::double precision / max_internal) +
        0.10 * (pr.outbound_internal_references::double precision / max_outbound) +
        0.05 * CASE
          WHEN r.publication_year IS NULL THEN 0
          ELSE 1.0 / (1.0 + (GREATEST(EXTRACT(YEAR FROM CURRENT_DATE)::int - r.publication_year, 0)::double precision / 10.0))
        END,
      metrics_computed_at = computed_at
  FROM reports r
  WHERE pr.project_id = target_project_id AND r.id = pr.report_id;

  SELECT metric_requested_revision INTO current_revision FROM projects WHERE id=target_project_id;

  SELECT count(*)::bigint
  INTO report_count
  FROM project_reports
  WHERE project_id = target_project_id;

  SELECT count(*)::bigint
  INTO edge_count
  FROM citations c
  JOIN project_reports source_membership
    ON source_membership.project_id = c.project_id
   AND source_membership.report_id = c.source_report_id
  JOIN project_reports target_membership
    ON target_membership.project_id = c.project_id
   AND target_membership.report_id = c.target_report_id
  WHERE c.project_id = target_project_id;

  INSERT INTO metric_snapshots
    (project_id, revision, metrics_as_of, work_count, edge_count, payload)
  VALUES
    (target_project_id, current_revision, computed_at, report_count, edge_count,
     jsonb_build_object('work_count', report_count, 'edge_count', edge_count))
  ON CONFLICT (project_id, revision) DO UPDATE SET
    metrics_as_of = EXCLUDED.metrics_as_of,
    work_count = EXCLUDED.work_count,
    edge_count = EXCLUDED.edge_count,
    payload = EXCLUDED.payload;

  UPDATE projection_state
  SET state = 'ready',
      revision = current_revision,
      watermark = current_revision,
      lag = 0,
      last_success_at = computed_at,
      last_error = NULL,
      rebuild_state = NULL,
      updated_at = computed_at
  WHERE projection_name = 'postgres_graph'
    AND project_id IS NOT DISTINCT FROM target_project_id;

  IF NOT FOUND THEN
    INSERT INTO projection_state
      (projection_name, project_id, state, revision, watermark, lag, last_success_at, updated_at)
    VALUES
      ('postgres_graph', target_project_id, 'ready', current_revision, current_revision, 0, computed_at, computed_at);
  END IF;
END;
$$;


--
-- Name: record_available_ai_opinions(uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.record_available_ai_opinions(project uuid, report uuid) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE target uuid;
BEGIN
 IF report IS NOT NULL THEN
   IF NOT ai_first_blinded(project,report) THEN
     PERFORM record_available_ai_opinions_legacy(project,report);
   END IF;
 ELSE
   FOR target IN SELECT report_id FROM project_reports WHERE project_id=project LOOP
     IF NOT ai_first_blinded(project,target) THEN
       PERFORM record_available_ai_opinions_legacy(project,target);
     END IF;
   END LOOP;
 END IF;
END $$;


--
-- Name: record_available_ai_opinions_legacy(uuid, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.record_available_ai_opinions_legacy(project uuid, report uuid) RETURNS void
    LANGUAGE sql
    AS $$
    INSERT INTO ai_opinion_exposures
      (project_id, report_id, stage, exposure_source, proposal_id, ai_run_id, exposure_possible_at)
    SELECT p.project_id, p.target_report_id, p.payload->>'stage', 'screening_suggestion',
           p.id, p.model_run_id, clock_timestamp()
    FROM ai_proposals p
    WHERE p.project_id=project AND (report IS NULL OR p.target_report_id=report)
      AND p.operation='screening_suggestion' AND p.target_report_id IS NOT NULL
      AND p.payload->>'stage' IN ('title_abstract', 'full_text')
      AND NOT ai_screening_proposal_withheld(p.project_id,p.id,p.target_report_id,p.payload->>'stage')
    ON CONFLICT DO NOTHING;

    INSERT INTO ai_opinion_exposures
      (project_id, report_id, stage, exposure_source, ai_reviewer_decision_id,
       proposal_id, ai_run_id, exposure_possible_at)
    SELECT d.project_id, d.report_id, d.stage, 'reviewer_opinion_reveal', d.id,
           d.proposal_id, d.ai_run_id, clock_timestamp()
    FROM ai_reviewer_decisions d
    LEFT JOIN screening_state s ON s.project_id=d.project_id AND s.report_id=d.report_id
    WHERE d.project_id=project AND (report IS NULL OR d.report_id=report)
      AND (d.resolved_at IS NOT NULL OR
           COALESCE(CASE WHEN d.stage='full_text' THEN s.full_text_status
                         ELSE s.title_abstract_status END, 'unscreened')
              NOT IN ('unscreened', 'not_required'))
    ON CONFLICT DO NOTHING
$$;


--
-- Name: redact_deleted_user_audit(uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.redact_deleted_user_audit(p_user_id uuid) RETURNS bigint
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'pg_catalog'
    AS $$
DECLARE
  changed bigint;
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM public.users u
    WHERE u.id = p_user_id
      AND (u.deleted_at IS NOT NULL OR EXISTS (
        SELECT 1 FROM public.account_deletion_requests r
        WHERE r.user_id = u.id AND r.state = 'completing'
      ))
  ) THEN
    RAISE EXCEPTION 'audit redaction requires a deleted user or a completing deletion request'
      USING ERRCODE = 'insufficient_privilege';
  END IF;

  -- Equivalent to SET LOCAL: this gate lasts only for the surrounding
  -- transaction and can name only the already-authorized user.
  PERFORM pg_catalog.set_config('deepref.audit_redact_user', p_user_id::text, true);

  UPDATE public.admin_audit_events
  SET metadata = metadata || pg_catalog.jsonb_build_object(
        'legacy_actor_id', 'redacted', 'source', 'redacted')
  WHERE target_user_id = p_user_id
    AND event_type IN ('legacy_actor.mapped', 'legacy_actor.non_identity_recorded')
    AND (metadata->>'legacy_actor_id' IS DISTINCT FROM 'redacted'
         OR metadata->>'source' IS DISTINCT FROM 'redacted');
  GET DIAGNOSTICS changed = ROW_COUNT;
  RETURN changed;
END;
$$;


--
-- Name: FUNCTION redact_deleted_user_audit(p_user_id uuid); Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON FUNCTION public.redact_deleted_user_audit(p_user_id uuid) IS 'Redacts named legacy-actor PII keys only for a deleted user or a user whose deletion request is completing.';


--
-- Name: reject_admin_audit_event_changes(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.reject_admin_audit_event_changes() RETURNS trigger
    LANGUAGE plpgsql
    SET search_path TO 'pg_catalog'
    AS $$
BEGIN
  IF TG_OP='DELETE'
     AND current_user::regrole=(SELECT proowner FROM pg_catalog.pg_proc WHERE oid='public.purge_retention_records(integer)'::regprocedure)
     AND current_setting('deepref.audit_purge',true)='on'
     AND OLD.occurred_at < now()-(CASE WHEN OLD.event_type='account.deletion_completed' THEN interval '5 years' ELSE interval '6 months' END)
  THEN RETURN OLD; END IF;
  IF TG_OP='UPDATE'
     AND current_user::regrole=(SELECT proowner FROM pg_catalog.pg_proc WHERE oid='public.redact_deleted_user_audit(uuid)'::regprocedure)
     AND current_setting('deepref.audit_redact_user',true)=OLD.target_user_id::text
     AND OLD.event_type IN ('legacy_actor.mapped','legacy_actor.non_identity_recorded')
     AND OLD.target_user_id IS NOT NULL
     AND EXISTS(SELECT 1 FROM public.users u WHERE u.id=OLD.target_user_id AND (u.deleted_at IS NOT NULL OR EXISTS(
       SELECT 1 FROM public.account_deletion_requests r WHERE r.user_id=u.id AND r.state='completing')))
     AND NEW.id=OLD.id AND NEW.occurred_at=OLD.occurred_at AND NEW.event_type=OLD.event_type
     AND NEW.principal_kind=OLD.principal_kind
     AND NEW.principal_user_id IS NOT DISTINCT FROM OLD.principal_user_id
     AND NEW.target_user_id IS NOT DISTINCT FROM OLD.target_user_id
     AND NEW.workspace_id IS NOT DISTINCT FROM OLD.workspace_id
     AND NEW.project_id IS NOT DISTINCT FROM OLD.project_id
     AND NEW.session_ref IS NOT DISTINCT FROM OLD.session_ref
     AND NEW.request_id IS NOT DISTINCT FROM OLD.request_id
     AND NEW.metadata-'legacy_actor_id'-'source'=OLD.metadata-'legacy_actor_id'-'source'
     AND NEW.metadata->>'legacy_actor_id'='redacted' AND NEW.metadata->>'source'='redacted'
  THEN RETURN NEW; END IF;
  RAISE EXCEPTION 'administrative audit events are append-only' USING ERRCODE='integrity_constraint_violation';
END;
$$;


--
-- Name: reject_ai_opinion_exposure_changes(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.reject_ai_opinion_exposure_changes() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  RAISE EXCEPTION 'AI opinion exposures are append-only'
    USING ERRCODE = 'integrity_constraint_violation';
END;
$$;


--
-- Name: reject_immutable_review_manifest_changes(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.reject_immutable_review_manifest_changes() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.project_id IS DISTINCT FROM OLD.project_id
      OR NEW.automation_run_id IS DISTINCT FROM OLD.automation_run_id
      OR NEW.definition_key IS DISTINCT FROM OLD.definition_key
      OR NEW.definition_id IS DISTINCT FROM OLD.definition_id
      OR NEW.definition_version IS DISTINCT FROM OLD.definition_version
      OR NEW.manifest_hash IS DISTINCT FROM OLD.manifest_hash
      OR NEW.semantic_bundle_hash IS DISTINCT FROM OLD.semantic_bundle_hash
      OR NEW.manifest IS DISTINCT FROM OLD.manifest
      OR NEW.subject IS DISTINCT FROM OLD.subject
      OR NEW.origin IS DISTINCT FROM OLD.origin
      OR NEW.prepared_task IS DISTINCT FROM OLD.prepared_task
      OR NEW.created_at IS DISTINCT FROM OLD.created_at THEN
    RAISE EXCEPTION 'review run manifest identity is immutable'
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NEW;
END;
$$;


--
-- Name: reject_legacy_calibration_identity(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.reject_legacy_calibration_identity() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF NEW.identity_scheme < 2 THEN
    RAISE EXCEPTION 'legacy calibration identity scheme 1 is read-only; new bundles need a component identity snapshot'
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NEW;
END;
$$;


--
-- Name: reject_review_artifact_changes(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.reject_review_artifact_changes() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  RAISE EXCEPTION 'review artifacts are immutable'
    USING ERRCODE = 'integrity_constraint_violation';
END;
$$;


--
-- Name: reject_review_calibration_bundle_changes(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.reject_review_calibration_bundle_changes() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  RAISE EXCEPTION 'review calibration bundles are immutable'
    USING ERRCODE = 'integrity_constraint_violation';
END;
$$;


--
-- Name: resolved_human_actor_user_id(text, text, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.resolved_human_actor_user_id(p_actor_kind text, p_actor_id text, p_actor_user_id uuid) RETURNS uuid
    LANGUAGE sql STABLE
    AS $$
  SELECT CASE WHEN p_actor_kind = 'user'
    THEN COALESCE(
      p_actor_user_id,
      legacy_human_actor_user_id(p_actor_kind, p_actor_id)
    )
  END
$$;


--
-- Name: same_human_actor_identity(text, text, text, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.same_human_actor_identity(p_left_kind text, p_left_id text, p_right_kind text, p_right_id text) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
  SELECT same_human_actor_identity(
    p_left_kind, p_left_id, NULL::uuid,
    p_right_kind, p_right_id, NULL::uuid
  )
$$;


--
-- Name: same_human_actor_identity(text, text, uuid, text, text, uuid); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.same_human_actor_identity(p_left_kind text, p_left_id text, p_left_user_id uuid, p_right_kind text, p_right_id text, p_right_user_id uuid) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$
  WITH identities AS (
    SELECT resolved_human_actor_user_id(
             p_left_kind, p_left_id, p_left_user_id
           ) AS left_user_id,
           resolved_human_actor_user_id(
             p_right_kind, p_right_id, p_right_user_id
           ) AS right_user_id
  )
  SELECT COALESCE(p_left_kind = 'user' AND p_right_kind = 'user'
    AND EXISTS (
      SELECT 1 FROM identities i
      WHERE i.left_user_id IS NULL OR i.right_user_id IS NULL
         OR i.left_user_id = i.right_user_id
    ), false)
$$;


--
-- Name: track_ai_opinion_availability(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.track_ai_opinion_availability() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_TABLE_NAME='ai_proposals' THEN
        IF NEW.operation='screening_suggestion' THEN
            PERFORM record_available_ai_opinions(NEW.project_id, NEW.target_report_id);
        END IF;
    ELSIF TG_TABLE_NAME='project_ai_autonomy' THEN
        IF NEW.task IN ('title_abstract_screening','full_text_screening') THEN
            PERFORM record_available_ai_opinions(NEW.project_id, NULL);
        END IF;
    ELSE
        PERFORM record_available_ai_opinions(NEW.project_id, NEW.report_id);
    END IF;
    RETURN NEW;
END;
$$;


--
-- Name: workflow_file_storage_guard(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.workflow_file_storage_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE current_mode text;
BEGIN
  SELECT mode INTO STRICT current_mode FROM workflow_file_storage_state WHERE singleton FOR SHARE;
  IF current_mode <> 'overlap' AND (current_setting('deepref.workflow_file_writer', true) IS DISTINCT FROM 'object-v1'
     OR NEW.storage_key IS NULL OR NEW.content_sha256 IS NULL OR NEW.byte_size IS NULL) THEN
    RAISE EXCEPTION 'workflow file object-only cutover rejects legacy writer';
  END IF;
  RETURN NEW;
END;
$$;


--
-- Name: workflow_versions_immutable(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.workflow_versions_immutable() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
  IF TG_OP = 'DELETE' THEN
    IF EXISTS (SELECT 1 FROM workflows WHERE id = OLD.workflow_id) THEN
      RAISE EXCEPTION 'published workflow versions cannot be deleted'
        USING ERRCODE = 'restrict_violation';
    END IF;
    RETURN OLD;
  END IF;
  RAISE EXCEPTION 'published workflow versions are immutable'
    USING ERRCODE = 'restrict_violation';
END;
$$;


SET LOCAL default_tablespace = '';

SET LOCAL default_table_access_method = heap;

--
-- Name: account_data_exports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_data_exports (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    status text NOT NULL,
    attempt_id uuid,
    job_id uuid NOT NULL,
    claim_owner text,
    claim_seq bigint,
    captured_workspaces uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    storage_key text,
    size_bytes bigint,
    sha256 text,
    retained_until timestamp with time zone,
    requested_at timestamp with time zone DEFAULT now() NOT NULL,
    processing_started_at timestamp with time zone,
    completed_at timestamp with time zone,
    failed_at timestamp with time zone,
    cancelled_at timestamp with time zone,
    error_message text,
    scope_manifest jsonb,
    CONSTRAINT account_data_exports_check CHECK (((status = 'processing'::text) = ((claim_owner IS NOT NULL) AND (claim_seq IS NOT NULL)))),
    CONSTRAINT account_data_exports_check1 CHECK (((status <> 'processing'::text) OR (attempt_id IS NOT NULL))),
    CONSTRAINT account_data_exports_check2 CHECK (((status <> 'completed'::text) OR ((storage_key IS NOT NULL) AND (size_bytes IS NOT NULL) AND (sha256 IS NOT NULL) AND (retained_until IS NOT NULL) AND (completed_at IS NOT NULL)))),
    CONSTRAINT account_data_exports_check3 CHECK (((status = 'failed'::text) = ((failed_at IS NOT NULL) AND (error_message IS NOT NULL)))),
    CONSTRAINT account_data_exports_check4 CHECK (((status = 'cancelled'::text) = (cancelled_at IS NOT NULL))),
    CONSTRAINT account_data_exports_sha256_check CHECK ((sha256 ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT account_data_exports_size_bytes_check CHECK ((size_bytes >= 0)),
    CONSTRAINT account_data_exports_status_check CHECK ((status = ANY (ARRAY['requested'::text, 'processing'::text, 'completed'::text, 'failed'::text, 'cancelled'::text, 'expired'::text])))
);


--
-- Name: account_deletion_requests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_deletion_requests (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    state text NOT NULL,
    requested_at timestamp with time zone DEFAULT now() NOT NULL,
    resolved_at timestamp with time zone,
    CONSTRAINT account_deletion_requests_check CHECK (((state = ANY (ARRAY['requested'::text, 'completing'::text])) = (resolved_at IS NULL))),
    CONSTRAINT account_deletion_requests_state_check CHECK ((state = ANY (ARRAY['requested'::text, 'cancelled'::text, 'completing'::text, 'completed'::text])))
);


--
-- Name: account_erasure_objects; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_erasure_objects (
    user_id uuid NOT NULL,
    storage_key text NOT NULL
);


--
-- Name: account_erasure_progress; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_erasure_progress (
    user_id uuid NOT NULL,
    due_at timestamp with time zone NOT NULL,
    started_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    email_hmac_sha256 text[] NOT NULL,
    logical_scrubbed_at timestamp with time zone
);


--
-- Name: account_erasure_rollout; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_erasure_rollout (
    singleton boolean DEFAULT true NOT NULL,
    activated boolean DEFAULT false NOT NULL,
    activated_at timestamp with time zone,
    attestation text,
    CONSTRAINT account_erasure_rollout_check CHECK (((NOT activated) OR ((activated_at IS NOT NULL) AND (attestation IS NOT NULL) AND (attestation = 'all-legacy-export-writers-stopped-and-untracked-staging-removed-no-restart'::text)))),
    CONSTRAINT account_erasure_rollout_singleton_check CHECK (singleton)
);


--
-- Name: account_erasure_writers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_erasure_writers (
    user_id uuid NOT NULL,
    attempt_id uuid NOT NULL
);


--
-- Name: account_export_rollout; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_export_rollout (
    singleton boolean DEFAULT true NOT NULL,
    retention_activated boolean DEFAULT false NOT NULL,
    activated_at timestamp with time zone,
    operator_attestation text,
    CONSTRAINT account_export_rollout_check CHECK ((((NOT retention_activated) AND (activated_at IS NULL) AND (operator_attestation IS NULL)) OR (retention_activated AND (activated_at IS NOT NULL) AND (operator_attestation IS NOT NULL) AND (operator_attestation = 'all-eager-deleting-binaries-drained-no-restart'::text)))),
    CONSTRAINT account_export_rollout_singleton_check CHECK (singleton)
);


--
-- Name: account_export_writers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_export_writers (
    attempt_id uuid NOT NULL,
    user_id uuid NOT NULL,
    export_id uuid NOT NULL,
    archive_key text NOT NULL,
    captured_workspaces uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    staging_directory text,
    upload_outcome text DEFAULT 'not_started'::text NOT NULL,
    upload_intent_at timestamp with time zone,
    upload_acknowledged_at timestamp with time zone,
    writer_owner text NOT NULL,
    claim_seq bigint NOT NULL,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    stopped_at timestamp with time zone,
    privacy_due_at timestamp with time zone,
    CONSTRAINT account_export_writers_check CHECK (((upload_outcome = 'not_started'::text) OR (upload_intent_at IS NOT NULL))),
    CONSTRAINT account_export_writers_check1 CHECK (((upload_outcome = 'acknowledged'::text) = (upload_acknowledged_at IS NOT NULL))),
    CONSTRAINT account_export_writers_claim_seq_check CHECK ((claim_seq > 0)),
    CONSTRAINT account_export_writers_staging_directory_check CHECK ((octet_length(staging_directory) <= 4096)),
    CONSTRAINT account_export_writers_upload_outcome_check CHECK ((upload_outcome = ANY (ARRAY['not_started'::text, 'dispatched'::text, 'acknowledged'::text, 'unresolved'::text])))
);


--
-- Name: account_human_activity; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.account_human_activity (
    user_id uuid NOT NULL,
    generation uuid NOT NULL,
    contact_generation uuid DEFAULT gen_random_uuid() NOT NULL,
    occurred_at timestamp with time zone NOT NULL,
    evidence_kind text NOT NULL,
    CONSTRAINT account_human_activity_evidence_kind_check CHECK ((evidence_kind = ANY (ARRAY['sign_in'::text, 'browser_interaction'::text, 'product_command'::text])))
);


--
-- Name: acquisition_items; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.acquisition_items (
    acquisition_id uuid NOT NULL,
    project_id uuid NOT NULL,
    report_id uuid NOT NULL,
    canonical_doi text NOT NULL,
    depth integer NOT NULL,
    parent_doi text,
    status text DEFAULT 'queued'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    last_error text,
    queued_at timestamp with time zone DEFAULT now() NOT NULL,
    fetched_at timestamp with time zone,
    work_event_id uuid,
    CONSTRAINT acquisition_items_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'fetching'::text, 'fetched'::text, 'skipped'::text, 'not_found'::text, 'failed'::text])))
);


--
-- Name: acquisition_runs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.acquisition_runs (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    source text DEFAULT 'acquisition'::text NOT NULL,
    status text DEFAULT 'queued'::text NOT NULL,
    max_depth integer DEFAULT 0 NOT NULL,
    seed_count integer DEFAULT 0 NOT NULL,
    queued_count integer DEFAULT 0 NOT NULL,
    fetched_count integer DEFAULT 0 NOT NULL,
    failed_count integer DEFAULT 0 NOT NULL,
    metadata_provider text DEFAULT ''::text NOT NULL,
    citation_provider text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone NOT NULL,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    strategy text DEFAULT 'citation_traversal'::text NOT NULL,
    format text,
    idempotency_key text,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    error text,
    historical_max_depth integer,
    CONSTRAINT acquisition_runs_depth_bounded CHECK ((((max_depth >= 0) AND (max_depth <= 3)) OR ((historical_max_depth IS NOT NULL) AND (max_depth = historical_max_depth))))
);


--
-- Name: admin_audit_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.admin_audit_events (
    id uuid NOT NULL,
    occurred_at timestamp with time zone DEFAULT now() NOT NULL,
    event_type text NOT NULL,
    principal_kind text NOT NULL,
    principal_user_id uuid,
    target_user_id uuid,
    session_ref bytea,
    request_id text,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    workspace_id uuid,
    project_id uuid,
    CONSTRAINT admin_audit_events_check CHECK ((((event_type = ANY (ARRAY['account.export_requested'::text, 'account.export_downloaded'::text, 'account.export_cancelled'::text])) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id IS NOT NULL) AND (target_user_id = principal_user_id) AND (workspace_id IS NULL) AND (project_id IS NULL) AND (metadata ? 'export_id'::text) AND (jsonb_typeof((metadata -> 'export_id'::text)) = 'string'::text) AND ((metadata - 'export_id'::text) = '{}'::jsonb) AND ((metadata ->> 'export_id'::text) ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) OR ((event_type = 'billing.changed'::text) AND (workspace_id IS NOT NULL) AND (project_id IS NULL) AND (metadata ? 'operation_id'::text) AND (metadata ? 'reason'::text) AND (((metadata - 'operation_id'::text) - 'reason'::text) = '{}'::jsonb) AND (jsonb_typeof((metadata -> 'operation_id'::text)) = 'string'::text) AND ((metadata ->> 'operation_id'::text) ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text) AND (jsonb_typeof((metadata -> 'reason'::text)) = 'string'::text) AND ((metadata ->> 'reason'::text) = ANY (ARRAY['provisioning_started'::text, 'customer_mapped'::text, 'checkout_created'::text, 'portal_created'::text, 'projection_updated'::text, 'review_required'::text, 'review_resolved'::text, 'free_enrolled'::text, 'detached'::text]))) OR ((event_type = ANY (ARRAY['auth.login'::text, 'auth.logout'::text, 'user.provisioned'::text])) AND (metadata = '{}'::jsonb) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR ((event_type = 'auth.session_revoked'::text) AND (metadata ? 'reason'::text) AND ((metadata - 'reason'::text) = '{}'::jsonb) AND ((metadata ->> 'reason'::text) = ANY (ARRAY['user_logout'::text, 'user_deactivated'::text])) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR ((event_type = 'user.deactivated'::text) AND (metadata ? 'source'::text) AND ((metadata - 'source'::text) = '{}'::jsonb) AND ((metadata ->> 'source'::text) = ANY (ARRAY['provider_event'::text, 'account_deletion'::text])) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR ((event_type = 'auth.rejected'::text) AND (principal_kind = 'system'::text) AND (principal_user_id IS NULL) AND (workspace_id IS NULL) AND (project_id IS NULL) AND (metadata ? 'stage'::text) AND (metadata ? 'reason'::text) AND (metadata ? 'window'::text) AND ((((metadata - 'stage'::text) - 'reason'::text) - 'window'::text) = '{}'::jsonb) AND ((metadata ->> 'window'::text) ~ '^[0-9]{1,10}$'::text) AND ((((metadata ->> 'stage'::text) = 'callback'::text) AND ((metadata ->> 'reason'::text) = ANY (ARRAY['invalid_state'::text, 'access_denied'::text, 'invalid_code'::text, 'provider_unavailable'::text, 'provisioning_failed'::text, 'account_deactivated'::text]))) OR (((metadata ->> 'stage'::text) = 'session'::text) AND ((metadata ->> 'reason'::text) = ANY (ARRAY['malformed_session_cookie'::text, 'unknown_expired_or_revoked_session'::text, 'session_user_mismatch'::text, 'session_reference_mismatch'::text, 'invalid_session'::text]))) OR (((metadata ->> 'stage'::text) = 'refresh'::text) AND ((metadata ->> 'reason'::text) = ANY (ARRAY['refresh_rejected'::text, 'session_revoked_during_refresh'::text, 'session_user_mismatch'::text, 'session_reference_mismatch'::text]))) OR (((metadata ->> 'stage'::text) = 'origin'::text) AND ((metadata ->> 'reason'::text) = ANY (ARRAY['multiple_origin_headers'::text, 'origin_mismatch'::text, 'sec_fetch_site_mismatch'::text, 'missing_origin_evidence'::text]))))) OR ((event_type = 'workspace.created'::text) AND (metadata ? 'workspace_kind'::text) AND ((metadata - 'workspace_kind'::text) = '{}'::jsonb) AND ((metadata ->> 'workspace_kind'::text) = ANY (ARRAY['personal'::text, 'organization'::text])) AND (workspace_id IS NOT NULL) AND (project_id IS NULL) AND (target_user_id IS NULL)) OR ((event_type = ANY (ARRAY['workspace.renamed'::text, 'workspace.deleted'::text])) AND (metadata = '{}'::jsonb) AND (workspace_id IS NOT NULL) AND (project_id IS NULL) AND (target_user_id IS NULL)) OR ((event_type = ANY (ARRAY['membership.created'::text, 'membership.removed'::text])) AND (metadata ? 'role'::text) AND (metadata ? 'grant_source'::text) AND (((metadata - 'role'::text) - 'grant_source'::text) = '{}'::jsonb) AND ((metadata ->> 'role'::text) = ANY (ARRAY['owner'::text, 'admin'::text, 'member'::text])) AND ((metadata ->> 'grant_source'::text) = ANY (ARRAY['workos_mirrored'::text, 'local'::text])) AND (workspace_id IS NOT NULL) AND (project_id IS NULL) AND (target_user_id IS NOT NULL)) OR ((event_type = 'membership.role_changed'::text) AND (metadata ? 'role'::text) AND ((metadata - 'role'::text) = '{}'::jsonb) AND ((metadata ->> 'role'::text) = ANY (ARRAY['owner'::text, 'admin'::text, 'member'::text])) AND (workspace_id IS NOT NULL) AND (project_id IS NULL) AND (target_user_id IS NOT NULL)) OR ((event_type = 'membership.status_changed'::text) AND (metadata ? 'status'::text) AND ((metadata - 'status'::text) = '{}'::jsonb) AND ((metadata ->> 'status'::text) = ANY (ARRAY['active'::text, 'deactivated'::text])) AND (workspace_id IS NOT NULL) AND (project_id IS NULL) AND (target_user_id IS NOT NULL)) OR ((event_type = ANY (ARRAY['project_access.granted'::text, 'project_access.removed'::text])) AND (metadata ? 'role'::text) AND (metadata ? 'grant_source'::text) AND (((metadata - 'role'::text) - 'grant_source'::text) = '{}'::jsonb) AND ((metadata ->> 'role'::text) = ANY (ARRAY['project_admin'::text, 'member'::text, 'viewer'::text])) AND ((metadata ->> 'grant_source'::text) = ANY (ARRAY['workos_mirrored'::text, 'local'::text])) AND (workspace_id IS NOT NULL) AND (project_id IS NOT NULL) AND (target_user_id IS NOT NULL)) OR ((event_type = 'project_access.role_changed'::text) AND (metadata ? 'role'::text) AND ((metadata - 'role'::text) = '{}'::jsonb) AND ((metadata ->> 'role'::text) = ANY (ARRAY['project_admin'::text, 'member'::text, 'viewer'::text])) AND (workspace_id IS NOT NULL) AND (project_id IS NOT NULL) AND (target_user_id IS NOT NULL)) OR ((event_type = ANY (ARRAY['project.created'::text, 'project.deleted'::text])) AND (metadata = '{}'::jsonb) AND (workspace_id IS NOT NULL) AND (project_id IS NOT NULL) AND (target_user_id IS NULL)) OR ((event_type = ANY (ARRAY['invitation.created'::text, 'invitation.revoked'::text, 'invitation.expired'::text])) AND (metadata ? 'role'::text) AND (metadata ? 'invitation_id'::text) AND (((metadata - 'role'::text) - 'invitation_id'::text) = '{}'::jsonb) AND ((metadata ->> 'role'::text) = ANY (ARRAY['owner'::text, 'admin'::text, 'member'::text])) AND ((metadata ->> 'invitation_id'::text) ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text) AND (workspace_id IS NOT NULL) AND (project_id IS NULL) AND (target_user_id IS NULL)) OR ((event_type = 'invitation.accepted'::text) AND (metadata ? 'role'::text) AND (metadata ? 'invitation_id'::text) AND (((metadata - 'role'::text) - 'invitation_id'::text) = '{}'::jsonb) AND ((metadata ->> 'role'::text) = ANY (ARRAY['owner'::text, 'admin'::text, 'member'::text])) AND ((metadata ->> 'invitation_id'::text) ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text) AND (workspace_id IS NOT NULL) AND (project_id IS NULL) AND (target_user_id IS NOT NULL)) OR ((event_type = 'authorization.denied'::text) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id IS NULL) AND (metadata ? 'permission'::text) AND (metadata ? 'resource_type'::text) AND (metadata ? 'window'::text) AND ((((metadata - 'permission'::text) - 'resource_type'::text) - 'window'::text) = '{}'::jsonb) AND ((metadata ->> 'permission'::text) = ANY (ARRAY['workspace:view'::text, 'workspace:manage'::text, 'workspace:billing'::text, 'workspace:members:invite'::text, 'project:create'::text, 'project:view'::text, 'project:edit'::text, 'project:delete'::text, 'screening:perform'::text, 'screening:resolve'::text, 'extraction:perform'::text, 'appraisal:perform'::text, 'automation:manage'::text, 'ai:use'::text, 'api_key:manage'::text, 'route_unlisted'::text, 'platform_operator'::text])) AND ((metadata ->> 'resource_type'::text) = ANY (ARRAY['project'::text, 'workspace'::text, 'none'::text])) AND ((metadata ->> 'window'::text) ~ '^[0-9]{1,10}$'::text) AND ((((metadata ->> 'resource_type'::text) = 'project'::text) AND (workspace_id IS NULL)) OR (((metadata ->> 'resource_type'::text) = 'workspace'::text) AND (project_id IS NULL)) OR (((metadata ->> 'resource_type'::text) = 'none'::text) AND (workspace_id IS NULL) AND (project_id IS NULL)))) OR ((event_type = ANY (ARRAY['legacy_actor.mapped'::text, 'legacy_actor.non_identity_recorded'::text])) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id IS NOT NULL) AND (workspace_id IS NULL) AND (project_id IS NULL) AND (metadata ? 'legacy_actor_kind'::text) AND (metadata ? 'legacy_actor_id'::text) AND (metadata ? 'source'::text) AND ((NOT (metadata ? 'affected_finalized_cohorts'::text)) OR (jsonb_typeof((metadata -> 'affected_finalized_cohorts'::text)) = 'array'::text)) AND (((((metadata - 'legacy_actor_kind'::text) - 'legacy_actor_id'::text) - 'source'::text) - 'affected_finalized_cohorts'::text) = '{}'::jsonb) AND ((metadata ->> 'legacy_actor_kind'::text) = 'user'::text) AND (btrim((metadata ->> 'legacy_actor_id'::text)) <> ''::text) AND (length((metadata ->> 'legacy_actor_id'::text)) <= 200) AND (btrim((metadata ->> 'source'::text)) <> ''::text) AND (length((metadata ->> 'source'::text)) <= 200)) OR ((event_type = ANY (ARRAY['account.deletion_requested'::text, 'account.deletion_cancelled'::text])) AND (metadata = '{}'::jsonb) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id = principal_user_id) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR ((event_type = 'account.deletion_completed'::text) AND (metadata = '{}'::jsonb) AND (principal_kind = 'system'::text) AND (principal_user_id IS NULL) AND (target_user_id IS NOT NULL) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR ((event_type = 'audit.purged'::text) AND (metadata ? 'rows_purged'::text) AND ((metadata - 'rows_purged'::text) = '{}'::jsonb) AND (jsonb_typeof((metadata -> 'rows_purged'::text)) = 'number'::text) AND ((metadata ->> 'rows_purged'::text) ~ '^[0-9]{1,10}$'::text) AND (workspace_id IS NULL) AND (project_id IS NULL) AND (target_user_id IS NULL)) OR ((event_type = 'ai.usage_limit_changed'::text) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id IS NULL) AND (metadata ? 'scope_kind'::text) AND (metadata ? 'old_limit_micros'::text) AND (metadata ? 'new_limit_micros'::text) AND ((((metadata - 'scope_kind'::text) - 'old_limit_micros'::text) - 'new_limit_micros'::text) = '{}'::jsonb) AND ((metadata ->> 'scope_kind'::text) = ANY (ARRAY['platform'::text, 'workspace'::text, 'project'::text])) AND (jsonb_typeof((metadata -> 'old_limit_micros'::text)) = 'number'::text) AND (jsonb_typeof((metadata -> 'new_limit_micros'::text)) = 'number'::text) AND (((metadata ->> 'old_limit_micros'::text))::numeric >= (0)::numeric) AND (((metadata ->> 'new_limit_micros'::text))::numeric >= (0)::numeric) AND ((((metadata ->> 'scope_kind'::text) = 'platform'::text) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR (((metadata ->> 'scope_kind'::text) = 'workspace'::text) AND (workspace_id IS NOT NULL) AND (project_id IS NULL)) OR (((metadata ->> 'scope_kind'::text) = 'project'::text) AND (workspace_id IS NOT NULL) AND (project_id IS NOT NULL)))) OR ((event_type = 'ai.usage_overage_waived'::text) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id IS NULL) AND (metadata ? 'overage_micros'::text) AND ((metadata - 'overage_micros'::text) = '{}'::jsonb) AND (jsonb_typeof((metadata -> 'overage_micros'::text)) = 'number'::text) AND (((metadata ->> 'overage_micros'::text))::numeric > (0)::numeric) AND (workspace_id IS NOT NULL) AND (project_id IS NULL)) OR ((event_type = 'ai.hard_caps_enabled'::text) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id IS NULL) AND (metadata ? 'quiet_window_seconds'::text) AND ((metadata - 'quiet_window_seconds'::text) = '{}'::jsonb) AND (jsonb_typeof((metadata -> 'quiet_window_seconds'::text)) = 'number'::text) AND (((metadata ->> 'quiet_window_seconds'::text))::numeric > (0)::numeric) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR ((event_type = 'ai.hard_caps_disabled'::text) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id IS NULL) AND ((metadata = '{}'::jsonb) OR ((metadata ? 'parked_jobs'::text) AND ((metadata - 'parked_jobs'::text) = '{}'::jsonb) AND (jsonb_typeof((metadata -> 'parked_jobs'::text)) = 'number'::text) AND (((metadata ->> 'parked_jobs'::text))::numeric >= (0)::numeric))) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR ((event_type = 'ai.usage_export_enabled'::text) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id IS NULL) AND (metadata ? 'quiet_window_seconds'::text) AND ((metadata - 'quiet_window_seconds'::text) = '{}'::jsonb) AND (jsonb_typeof((metadata -> 'quiet_window_seconds'::text)) = 'number'::text) AND (((metadata ->> 'quiet_window_seconds'::text))::numeric > (0)::numeric) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR ((event_type = 'ai.usage_export_disabled'::text) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id IS NULL) AND (metadata ? 'parked_jobs'::text) AND ((metadata - 'parked_jobs'::text) = '{}'::jsonb) AND (jsonb_typeof((metadata -> 'parked_jobs'::text)) = 'number'::text) AND (((metadata ->> 'parked_jobs'::text))::numeric >= (0)::numeric) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR ((event_type = 'ai.usage_reservation_resolved'::text) AND (principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL) AND (target_user_id IS NULL) AND (metadata ? 'reservation_id'::text) AND (metadata ? 'resolution'::text) AND ((metadata ->> 'reservation_id'::text) ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text) AND ((metadata ->> 'resolution'::text) = ANY (ARRAY['settle'::text, 'void'::text])) AND ((((metadata ->> 'resolution'::text) = 'settle'::text) AND (metadata ? 'cost_micros'::text) AND ((((metadata - 'reservation_id'::text) - 'resolution'::text) - 'cost_micros'::text) = '{}'::jsonb) AND (jsonb_typeof((metadata -> 'cost_micros'::text)) = 'number'::text) AND (((metadata ->> 'cost_micros'::text))::numeric >= (0)::numeric)) OR (((metadata ->> 'resolution'::text) = 'void'::text) AND (metadata ? 'reason'::text) AND ((((metadata - 'reservation_id'::text) - 'resolution'::text) - 'reason'::text) = '{}'::jsonb) AND (jsonb_typeof((metadata -> 'reason'::text)) = 'string'::text) AND ((length(btrim((metadata ->> 'reason'::text))) >= 1) AND (length(btrim((metadata ->> 'reason'::text))) <= 500)))) AND (workspace_id IS NOT NULL)) OR ((event_type = ANY (ARRAY['api_key.created'::text, 'api_key.revoked'::text, 'api_key.rotated'::text])) AND (((principal_kind = 'user'::text) AND (principal_user_id IS NOT NULL)) OR ((event_type = 'api_key.revoked'::text) AND (principal_kind = 'system'::text) AND (principal_user_id IS NULL))) AND (workspace_id IS NOT NULL) AND (project_id IS NULL) AND (target_user_id IS NULL) AND (metadata ? 'key_id'::text) AND ((metadata - 'key_id'::text) = '{}'::jsonb) AND (jsonb_typeof((metadata -> 'key_id'::text)) = 'string'::text) AND ((metadata ->> 'key_id'::text) ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) OR ((event_type = 'auth.rejected'::text) AND (principal_kind = 'system'::text) AND (principal_user_id IS NULL) AND (target_user_id IS NULL) AND (workspace_id IS NULL) AND (project_id IS NULL) AND (metadata ? 'stage'::text) AND (metadata ? 'reason'::text) AND (metadata ? 'window'::text) AND ((((metadata - 'stage'::text) - 'reason'::text) - 'window'::text) = '{}'::jsonb) AND ((metadata ->> 'stage'::text) = 'api_key'::text) AND ((metadata ->> 'reason'::text) = ANY (ARRAY['invalid_credential'::text, 'service_route_denied'::text])) AND ((metadata ->> 'window'::text) ~ '^[0-9]{1,10}$'::text)))),
    CONSTRAINT admin_audit_events_check1 CHECK (((principal_kind = 'user'::text) = (principal_user_id IS NOT NULL))),
    CONSTRAINT admin_audit_events_event_type_check CHECK ((event_type = ANY (ARRAY['account.export_requested'::text, 'account.export_downloaded'::text, 'account.export_cancelled'::text, 'billing.changed'::text, 'api_key.created'::text, 'api_key.revoked'::text, 'api_key.rotated'::text, 'auth.login'::text, 'auth.logout'::text, 'auth.rejected'::text, 'auth.session_revoked'::text, 'user.provisioned'::text, 'user.deactivated'::text, 'workspace.created'::text, 'workspace.renamed'::text, 'workspace.deleted'::text, 'membership.created'::text, 'membership.role_changed'::text, 'membership.status_changed'::text, 'membership.removed'::text, 'project_access.granted'::text, 'project_access.role_changed'::text, 'project_access.removed'::text, 'project.created'::text, 'project.deleted'::text, 'invitation.created'::text, 'invitation.accepted'::text, 'invitation.revoked'::text, 'invitation.expired'::text, 'authorization.denied'::text, 'legacy_actor.mapped'::text, 'legacy_actor.non_identity_recorded'::text, 'account.deletion_requested'::text, 'account.deletion_cancelled'::text, 'account.deletion_completed'::text, 'audit.purged'::text, 'ai.usage_limit_changed'::text, 'ai.usage_overage_waived'::text, 'ai.hard_caps_enabled'::text, 'ai.hard_caps_disabled'::text, 'ai.usage_reservation_resolved'::text, 'ai.usage_export_enabled'::text, 'ai.usage_export_disabled'::text]))),
    CONSTRAINT admin_audit_events_metadata_check CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT admin_audit_events_principal_kind_check CHECK ((principal_kind = ANY (ARRAY['user'::text, 'system'::text]))),
    CONSTRAINT admin_audit_events_request_id_check CHECK (((request_id IS NULL) OR (length(request_id) <= 200))),
    CONSTRAINT admin_audit_events_session_ref_check CHECK (((session_ref IS NULL) OR (length(session_ref) = 32)))
);


--
-- Name: COLUMN admin_audit_events.workspace_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.admin_audit_events.workspace_id IS 'The workspace a tenancy event changed. Identifier only, never a foreign key: append-only rows outlive workspaces.';


--
-- Name: COLUMN admin_audit_events.project_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.admin_audit_events.project_id IS 'The project a project-access event changed. Identifier only, never a foreign key: append-only rows outlive projects.';


--
-- Name: ai_activity; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_activity (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    actor_type text NOT NULL,
    actor_label text NOT NULL,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    task text NOT NULL,
    action text NOT NULL,
    summary text NOT NULL,
    affected jsonb DEFAULT '[]'::jsonb NOT NULL,
    before_state jsonb DEFAULT '{}'::jsonb NOT NULL,
    after_state jsonb DEFAULT '{}'::jsonb NOT NULL,
    undo_kind text,
    batch_id uuid,
    ai_run_id uuid,
    proposal_id uuid,
    model text,
    prompt_version text,
    evidence jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    undone_at timestamp with time zone,
    undone_by_kind text,
    undone_by_id text,
    CONSTRAINT ai_activity_action_check CHECK ((length(btrim(action)) > 0)),
    CONSTRAINT ai_activity_actor_label_check CHECK ((length(btrim(actor_label)) > 0)),
    CONSTRAINT ai_activity_actor_type_check CHECK ((actor_type = ANY (ARRAY['ai'::text, 'automation'::text, 'assistant'::text]))),
    CONSTRAINT ai_activity_affected_check CHECK ((jsonb_typeof(affected) = 'array'::text)),
    CONSTRAINT ai_activity_evidence_check CHECK ((jsonb_typeof(evidence) = 'array'::text)),
    CONSTRAINT ai_activity_summary_check CHECK ((length(btrim(summary)) > 0)),
    CONSTRAINT ai_activity_task_check CHECK ((length(btrim(task)) > 0)),
    CONSTRAINT ai_activity_undo_kind_check CHECK (((undo_kind IS NULL) OR (undo_kind = ANY (ARRAY['extraction_values'::text, 'duplicate_link'::text, 'screening_event'::text, 'reviewer_decision'::text])))),
    CONSTRAINT ai_activity_undone_shape CHECK ((((undone_at IS NULL) AND (undone_by_kind IS NULL) AND (undone_by_id IS NULL)) OR ((undone_at IS NOT NULL) AND (undone_by_kind IS NOT NULL) AND (undone_by_id IS NOT NULL))))
);


--
-- Name: ai_autonomy_outbox; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_autonomy_outbox (
    project_id uuid NOT NULL,
    proposal_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    last_error text,
    CONSTRAINT ai_autonomy_outbox_attempts_check CHECK ((attempts >= 0))
);


--
-- Name: ai_concurrency_slots; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_concurrency_slots (
    scope_kind text NOT NULL,
    scope_key text NOT NULL,
    slot_index integer NOT NULL,
    holder_id uuid NOT NULL,
    generation bigint NOT NULL,
    acquired_at timestamp with time zone NOT NULL,
    lease_expires_at timestamp with time zone NOT NULL,
    CONSTRAINT ai_concurrency_slots_scope_kind_check CHECK ((scope_kind = ANY (ARRAY['workspace'::text, 'route'::text]))),
    CONSTRAINT ai_concurrency_slots_slot_index_check CHECK ((slot_index >= 0))
);


--
-- Name: ai_concurrency_slots_generation_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.ai_concurrency_slots_generation_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: ai_model_routes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_model_routes (
    id uuid NOT NULL,
    profile text NOT NULL,
    provider text NOT NULL,
    model text NOT NULL,
    model_version text NOT NULL,
    parameters jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    effective_from timestamp with time zone DEFAULT now() NOT NULL,
    effective_until timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_model_routes_check CHECK (((effective_until IS NULL) OR (effective_until > effective_from))),
    CONSTRAINT ai_model_routes_model_check CHECK ((length(btrim(model)) > 0)),
    CONSTRAINT ai_model_routes_model_version_check CHECK ((length(btrim(model_version)) > 0)),
    CONSTRAINT ai_model_routes_parameters_check CHECK ((jsonb_typeof(parameters) = 'object'::text)),
    CONSTRAINT ai_model_routes_profile_check CHECK ((length(btrim(profile)) > 0)),
    CONSTRAINT ai_model_routes_provider_check CHECK ((length(btrim(provider)) > 0))
);


--
-- Name: ai_opinion_exposures; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_opinion_exposures (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    report_id uuid NOT NULL,
    stage text NOT NULL,
    exposure_source text NOT NULL,
    audience_actor_kind text,
    audience_actor_id text,
    ai_reviewer_decision_id uuid,
    proposal_id uuid,
    ai_run_id uuid,
    workflow_run_id uuid,
    exposure_possible_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    audience_actor_user_id uuid,
    CONSTRAINT ai_opinion_exposures_audience_actor_id_check CHECK (((audience_actor_id IS NULL) OR (length(btrim(audience_actor_id)) > 0))),
    CONSTRAINT ai_opinion_exposures_audience_actor_kind_check CHECK (((audience_actor_kind IS NULL) OR (audience_actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text])))),
    CONSTRAINT ai_opinion_exposures_audience_shape CHECK ((((audience_actor_kind IS NULL) AND (audience_actor_id IS NULL)) OR ((audience_actor_kind IS NOT NULL) AND (audience_actor_id IS NOT NULL)))),
    CONSTRAINT ai_opinion_exposures_exposure_source_check CHECK ((exposure_source = ANY (ARRAY['workflow_run_output'::text, 'screening_suggestion'::text, 'reviewer_opinion_reveal'::text, 'activity_feed_legacy'::text, 'proposal_legacy'::text]))),
    CONSTRAINT ai_opinion_exposures_stage_check CHECK ((stage = ANY (ARRAY['title_abstract'::text, 'full_text'::text])))
);


--
-- Name: ai_proposal_criterion_judgments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_proposal_criterion_judgments (
    proposal_id uuid NOT NULL,
    project_id uuid NOT NULL,
    criterion_id uuid NOT NULL,
    protocol_version_id uuid NOT NULL,
    ordinal integer NOT NULL,
    judgment text NOT NULL,
    rationale text NOT NULL,
    evidence jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_proposal_criterion_judgments_evidence_check CHECK ((jsonb_typeof(evidence) = 'array'::text)),
    CONSTRAINT ai_proposal_criterion_judgments_judgment_check CHECK ((judgment = ANY (ARRAY['meets'::text, 'does_not_meet'::text, 'unclear'::text]))),
    CONSTRAINT ai_proposal_criterion_judgments_ordinal_check CHECK ((ordinal >= 0)),
    CONSTRAINT ai_proposal_criterion_judgments_rationale_check CHECK ((length(btrim(rationale)) > 0))
);


--
-- Name: ai_proposal_evidence; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_proposal_evidence (
    proposal_id uuid NOT NULL,
    project_id uuid NOT NULL,
    ordinal integer NOT NULL,
    evidence_kind text NOT NULL,
    report_id uuid NOT NULL,
    document_id uuid,
    document_block_id uuid,
    page integer,
    source_field text,
    content_hash text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_proposal_evidence_check CHECK ((((evidence_kind = 'report_metadata'::text) AND (source_field IS NOT NULL) AND (document_id IS NULL) AND (document_block_id IS NULL) AND (page IS NULL)) OR ((evidence_kind = 'document_block'::text) AND (source_field IS NULL) AND (document_id IS NOT NULL) AND (document_block_id IS NOT NULL) AND (page IS NOT NULL)))),
    CONSTRAINT ai_proposal_evidence_content_hash_check CHECK ((content_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT ai_proposal_evidence_evidence_kind_check CHECK ((evidence_kind = ANY (ARRAY['report_metadata'::text, 'document_block'::text]))),
    CONSTRAINT ai_proposal_evidence_ordinal_check CHECK ((ordinal >= 0)),
    CONSTRAINT ai_proposal_evidence_page_check CHECK (((page IS NULL) OR (page > 0))),
    CONSTRAINT ai_proposal_evidence_source_field_check CHECK (((source_field IS NULL) OR (source_field = ANY (ARRAY['title'::text, 'abstract'::text]))))
);


--
-- Name: ai_proposals; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_proposals (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    ai_run_id uuid NOT NULL,
    proposal_type text NOT NULL,
    payload jsonb NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    decided_by text,
    decided_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    entity_type text NOT NULL,
    entity_id uuid,
    operation text NOT NULL,
    model_run_id uuid NOT NULL,
    authority_tier text NOT NULL,
    resolved_at timestamp with time zone,
    resolved_by_actor_kind text,
    resolved_by_actor_id text,
    resolution_reason text,
    task_kind text NOT NULL,
    target_report_id uuid,
    target_record_id uuid,
    protocol_version_id uuid,
    expected_revision bigint,
    target_study_id uuid,
    CONSTRAINT ai_proposals_authority_tier_check CHECK ((authority_tier = ANY (ARRAY['read_only'::text, 'reversible_metadata'::text, 'workflow_suggestion'::text, 'scientific_conclusion'::text]))),
    CONSTRAINT ai_proposals_expected_revision_check CHECK (((expected_revision IS NULL) OR (expected_revision >= 0))),
    CONSTRAINT ai_proposals_payload_object_check CHECK ((jsonb_typeof(payload) = 'object'::text)),
    CONSTRAINT ai_proposals_resolution_actor_shape_check CHECK ((((status = 'pending'::text) AND (resolved_at IS NULL) AND (resolved_by_actor_kind IS NULL) AND (resolved_by_actor_id IS NULL)) OR ((status <> 'pending'::text) AND (resolved_at IS NOT NULL) AND (resolved_by_actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text])) AND (resolved_by_actor_id IS NOT NULL) AND (length(btrim(resolved_by_actor_id)) > 0)))),
    CONSTRAINT ai_proposals_run_alias_check CHECK ((ai_run_id = model_run_id)),
    CONSTRAINT ai_proposals_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'accepted'::text, 'rejected'::text, 'expired'::text]))),
    CONSTRAINT ai_proposals_task_kind_check CHECK ((length(btrim(task_kind)) > 0))
);


--
-- Name: ai_reviewer_decisions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_reviewer_decisions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    report_id uuid NOT NULL,
    stage text NOT NULL,
    decision text NOT NULL,
    rationale text NOT NULL,
    evidence jsonb DEFAULT '[]'::jsonb NOT NULL,
    source text DEFAULT 'ai'::text NOT NULL,
    proposal_id uuid,
    ai_run_id uuid,
    model text,
    prompt_version text,
    activity_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    voided_at timestamp with time zone,
    human_decision_before text,
    resolved_at timestamp with time zone,
    resolved_by_kind text,
    resolved_by_id text,
    resolution text,
    resolution_note text,
    human_decision_before_at timestamp with time zone,
    CONSTRAINT ai_reviewer_decisions_decision_check CHECK ((decision = ANY (ARRAY['include'::text, 'exclude'::text, 'maybe'::text]))),
    CONSTRAINT ai_reviewer_decisions_evidence_check CHECK ((jsonb_typeof(evidence) = 'array'::text)),
    CONSTRAINT ai_reviewer_decisions_rationale_check CHECK ((length(btrim(rationale)) > 0)),
    CONSTRAINT ai_reviewer_decisions_resolution_check CHECK (((resolution IS NULL) OR (resolution = ANY (ARRAY['kept_human'::text, 'adopted_ai'::text, 'other'::text])))),
    CONSTRAINT ai_reviewer_decisions_source_check CHECK ((source = ANY (ARRAY['ai'::text, 'workflow'::text]))),
    CONSTRAINT ai_reviewer_decisions_stage_check CHECK ((stage = ANY (ARRAY['title_abstract'::text, 'full_text'::text])))
);


--
-- Name: ai_run_evidence; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_run_evidence (
    ai_run_id uuid NOT NULL,
    project_id uuid NOT NULL,
    document_id uuid NOT NULL,
    document_block_id uuid NOT NULL,
    rank integer NOT NULL,
    retrieval_score double precision NOT NULL,
    content_hash text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_run_evidence_content_hash_check CHECK ((content_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT ai_run_evidence_rank_check CHECK ((rank > 0)),
    CONSTRAINT ai_run_evidence_retrieval_score_check CHECK ((retrieval_score >= (0)::double precision))
);


--
-- Name: ai_runs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_runs (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    task_kind text NOT NULL,
    provider text NOT NULL,
    model text NOT NULL,
    prompt_version text NOT NULL,
    input_hash text NOT NULL,
    output jsonb,
    status text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone,
    profile text NOT NULL,
    model_version text NOT NULL,
    parameters jsonb DEFAULT '{}'::jsonb NOT NULL,
    schema_version text NOT NULL,
    prompt_hash text NOT NULL,
    schema_hash text NOT NULL,
    reuse_hash text NOT NULL,
    protocol_hash text,
    document_hash text,
    evidence_hash text,
    evidence_refs jsonb DEFAULT '[]'::jsonb NOT NULL,
    input_tokens bigint DEFAULT 0 NOT NULL,
    output_tokens bigint DEFAULT 0 NOT NULL,
    cost_micros bigint,
    error_code text,
    error_message text,
    parent_automation_run_id uuid,
    provider_served_model text,
    provider_system_fingerprint text,
    CONSTRAINT ai_runs_evidence_refs_array_check CHECK ((jsonb_typeof(evidence_refs) = 'array'::text)),
    CONSTRAINT ai_runs_input_tokens_check CHECK ((input_tokens >= 0)),
    CONSTRAINT ai_runs_output_tokens_check CHECK ((output_tokens >= 0)),
    CONSTRAINT ai_runs_parameters_object_check CHECK ((jsonb_typeof(parameters) = 'object'::text)),
    CONSTRAINT ai_runs_profile_check CHECK ((length(btrim(profile)) > 0)),
    CONSTRAINT ai_runs_prompt_hash_check CHECK ((prompt_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT ai_runs_provider_served_model_check CHECK (((provider_served_model IS NULL) OR ((char_length(provider_served_model) >= 1) AND (char_length(provider_served_model) <= 128)))),
    CONSTRAINT ai_runs_provider_system_fingerprint_check CHECK (((provider_system_fingerprint IS NULL) OR ((char_length(provider_system_fingerprint) >= 1) AND (char_length(provider_system_fingerprint) <= 128)))),
    CONSTRAINT ai_runs_reuse_hash_check CHECK ((reuse_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT ai_runs_schema_hash_check CHECK ((schema_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT ai_runs_status_check CHECK ((status = ANY (ARRAY['running'::text, 'completed'::text, 'failed'::text, 'abstained'::text])))
);


--
-- Name: ai_screening_audit_labels; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_screening_audit_labels (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    cohort_id uuid NOT NULL,
    report_id uuid NOT NULL,
    actor_id text NOT NULL,
    decision text NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    actor_user_id uuid,
    CONSTRAINT ai_screening_audit_labels_decision_check CHECK ((decision = ANY (ARRAY['include'::text, 'exclude'::text, 'maybe'::text])))
);


--
-- Name: ai_screening_cohort_members; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_screening_cohort_members (
    project_id uuid NOT NULL,
    cohort_id uuid NOT NULL,
    report_id uuid NOT NULL,
    evaluated_revision bigint NOT NULL,
    review_run_id uuid,
    sampled boolean DEFAULT false NOT NULL,
    quarantine_frame boolean DEFAULT false NOT NULL,
    audit_control boolean DEFAULT false NOT NULL,
    audit_order integer,
    CONSTRAINT ai_screening_cohort_members_audit_order_check CHECK ((audit_order >= 0)),
    CONSTRAINT ai_screening_cohort_members_check CHECK ((NOT (audit_control AND quarantine_frame))),
    CONSTRAINT ai_screening_cohort_members_evaluated_revision_check CHECK ((evaluated_revision >= 0))
);


--
-- Name: ai_screening_cohorts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_screening_cohorts (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    stage text DEFAULT 'title_abstract'::text NOT NULL,
    protocol_version_id uuid NOT NULL,
    semantic_bundle_hash text NOT NULL,
    semantic_identity jsonb NOT NULL,
    policy_version integer NOT NULL,
    status text DEFAULT 'open'::text NOT NULL,
    target_percent integer NOT NULL,
    audit_ordinal integer,
    alpha_billionths integer,
    reference_relevant integer,
    population integer,
    sample_size integer,
    sampling_seed uuid,
    sampling_algorithm text,
    reference_snapshot jsonb,
    frame_snapshot jsonb,
    result jsonb,
    approved_by text NOT NULL,
    final_approval_id uuid,
    finalized_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    closed_at timestamp with time zone,
    evaluated_at timestamp with time zone,
    finalized_at timestamp with time zone,
    invalidation_reason text,
    identity_flagged_at timestamp with time zone,
    identity_flag_reason text,
    approved_by_actor_user_id uuid,
    finalized_by_actor_user_id uuid,
    CONSTRAINT ai_screening_cohorts_alpha_billionths_check CHECK (((alpha_billionths >= 1) AND (alpha_billionths <= 50000000))),
    CONSTRAINT ai_screening_cohorts_check CHECK ((((sampling_seed IS NULL) AND (audit_ordinal IS NULL) AND (sample_size IS NULL)) OR ((sampling_seed IS NOT NULL) AND (audit_ordinal IS NOT NULL) AND (sample_size IS NOT NULL) AND (population IS NOT NULL) AND (reference_relevant IS NOT NULL) AND (alpha_billionths IS NOT NULL) AND (sampling_algorithm IS NOT NULL) AND (sample_size <= population)))),
    CONSTRAINT ai_screening_cohorts_check1 CHECK (((final_approval_id IS NULL) = (finalized_by IS NULL))),
    CONSTRAINT ai_screening_cohorts_identity_flag_check CHECK (((identity_flagged_at IS NULL) = (identity_flag_reason IS NULL))),
    CONSTRAINT ai_screening_cohorts_policy_version_check CHECK ((policy_version = 1)),
    CONSTRAINT ai_screening_cohorts_population_check CHECK ((population >= 0)),
    CONSTRAINT ai_screening_cohorts_reference_relevant_check CHECK ((reference_relevant >= 0)),
    CONSTRAINT ai_screening_cohorts_sample_size_check CHECK ((sample_size >= 0)),
    CONSTRAINT ai_screening_cohorts_semantic_bundle_hash_check CHECK ((length(semantic_bundle_hash) = 64)),
    CONSTRAINT ai_screening_cohorts_stage_check CHECK ((stage = 'title_abstract'::text)),
    CONSTRAINT ai_screening_cohorts_status_check CHECK ((status = ANY (ARRAY['open'::text, 'closed'::text, 'auditing'::text, 'passed'::text, 'failed'::text, 'invalidated'::text, 'declined'::text, 'finalized'::text]))),
    CONSTRAINT ai_screening_cohorts_target_percent_check CHECK ((target_percent = ANY (ARRAY[95, 98])))
);


--
-- Name: ai_screening_dispositions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_screening_dispositions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    cohort_id uuid NOT NULL,
    report_id uuid NOT NULL,
    stage text DEFAULT 'title_abstract'::text NOT NULL,
    kind text DEFAULT 'quarantine_exclude'::text NOT NULL,
    review_run_id uuid NOT NULL,
    ai_run_id uuid NOT NULL,
    evaluated_revision bigint NOT NULL,
    protocol_version_id uuid NOT NULL,
    semantic_bundle_hash text NOT NULL,
    policy_version integer NOT NULL,
    source_snapshot jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    voided_at timestamp with time zone,
    void_reason text,
    finalized_at timestamp with time zone,
    finalized_event_id uuid,
    CONSTRAINT ai_screening_dispositions_check CHECK ((NOT ((voided_at IS NOT NULL) AND (finalized_at IS NOT NULL)))),
    CONSTRAINT ai_screening_dispositions_check1 CHECK (((voided_at IS NULL) = (void_reason IS NULL))),
    CONSTRAINT ai_screening_dispositions_check2 CHECK (((finalized_at IS NULL) = (finalized_event_id IS NULL))),
    CONSTRAINT ai_screening_dispositions_evaluated_revision_check CHECK ((evaluated_revision >= 0)),
    CONSTRAINT ai_screening_dispositions_kind_check CHECK ((kind = 'quarantine_exclude'::text)),
    CONSTRAINT ai_screening_dispositions_policy_version_check CHECK ((policy_version = 1)),
    CONSTRAINT ai_screening_dispositions_semantic_bundle_hash_check CHECK ((length(semantic_bundle_hash) = 64)),
    CONSTRAINT ai_screening_dispositions_stage_check CHECK ((stage = 'title_abstract'::text))
);


--
-- Name: ai_usage_backfill_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_backfill_state (
    singleton boolean DEFAULT true NOT NULL,
    completed_at timestamp with time zone,
    processed_rows bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_usage_backfill_state_processed_rows_check CHECK ((processed_rows >= 0)),
    CONSTRAINT ai_usage_backfill_state_singleton_check CHECK (singleton)
);


--
-- Name: ai_usage_corrections; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_corrections (
    id uuid NOT NULL,
    ledger_id uuid NOT NULL,
    original_cost_micros bigint NOT NULL,
    corrected_cost_micros bigint NOT NULL,
    original_price_version text NOT NULL,
    corrected_price_version text NOT NULL,
    reason text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_usage_corrections_corrected_cost_micros_check CHECK ((corrected_cost_micros >= 0)),
    CONSTRAINT ai_usage_corrections_original_cost_micros_check CHECK ((original_cost_micros >= 0)),
    CONSTRAINT ai_usage_corrections_reason_check CHECK (((length(reason) >= 1) AND (length(reason) <= 500)))
);


--
-- Name: COLUMN ai_usage_corrections.id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.ai_usage_corrections.id IS 'Immutable correction fact identity; also its export event identity.';


--
-- Name: COLUMN ai_usage_corrections.ledger_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.ai_usage_corrections.ledger_id IS 'Original usage fact this correction references. The original row is never edited.';


--
-- Name: ai_usage_detached_projects; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_detached_projects (
    project_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    detached_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: ai_usage_fact_ids; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_fact_ids (
    fact_id uuid NOT NULL,
    fact_kind text NOT NULL,
    recorded_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_usage_fact_ids_fact_kind_check CHECK ((fact_kind = ANY (ARRAY['usage'::text, 'correction'::text])))
);


--
-- Name: TABLE ai_usage_fact_ids; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.ai_usage_fact_ids IS 'The shared identity namespace of usage facts and correction records. A fact id is also its export event id and is never reissued or rewritten.';


--
-- Name: COLUMN ai_usage_fact_ids.fact_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.ai_usage_fact_ids.fact_id IS 'Immutable fact identity; also the export event identity of this fact.';


--
-- Name: ai_usage_ledger; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_ledger (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid,
    profile text NOT NULL,
    provider text NOT NULL,
    model text NOT NULL,
    purpose text NOT NULL,
    input_tokens bigint,
    output_tokens bigint,
    cost_micros bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    workspace_id uuid,
    principal_kind text,
    principal_id text,
    run_id uuid,
    run_budget_id uuid,
    step_id text,
    reasoning_tokens bigint,
    commercial_units bigint,
    price_version text,
    current_price_version text,
    pricing_decision text,
    period_start date,
    reservation_id uuid,
    provider_request_id text,
    provider_response_id text,
    CONSTRAINT ai_usage_ledger_commercial_units_check CHECK (((commercial_units IS NULL) OR (commercial_units >= 0))),
    CONSTRAINT ai_usage_ledger_cost_micros_check CHECK ((cost_micros >= 0)),
    CONSTRAINT ai_usage_ledger_input_tokens_check CHECK ((input_tokens >= 0)),
    CONSTRAINT ai_usage_ledger_output_tokens_check CHECK ((output_tokens >= 0)),
    CONSTRAINT ai_usage_ledger_principal_check CHECK ((((principal_kind IS NULL) = (principal_id IS NULL)) AND ((principal_kind IS NULL) OR (principal_kind = ANY (ARRAY['user'::text, 'service'::text]))))),
    CONSTRAINT ai_usage_ledger_purpose_check CHECK ((purpose = ANY (ARRAY['structured'::text, 'chat'::text, 'assistant'::text]))),
    CONSTRAINT ai_usage_ledger_reasoning_tokens_check CHECK ((reasoning_tokens >= 0)),
    CONSTRAINT ai_usage_ledger_step_id_check CHECK (((step_id IS NULL) OR ((length(step_id) >= 1) AND (length(step_id) <= 200))))
);


--
-- Name: COLUMN ai_usage_ledger.cost_micros; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.ai_usage_ledger.cost_micros IS 'Provider cost in micro-dollars. Kept separate from nullable commercial_units.';


--
-- Name: COLUMN ai_usage_ledger.workspace_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.ai_usage_ledger.workspace_id IS 'Immutable tenant identity retained with the five-year usage fact; not a foreign key.';


--
-- Name: COLUMN ai_usage_ledger.commercial_units; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.ai_usage_ledger.commercial_units IS 'Provider-neutral commercial meter quantity. NULL until a commercial meter is selected.';


--
-- Name: COLUMN ai_usage_ledger.period_start; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.ai_usage_ledger.period_start IS 'UTC month charged by this usage fact. A fact has exactly one period.';


--
-- Name: ai_usage_legacy_backfill; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_legacy_backfill (
    ledger_id uuid NOT NULL,
    processed_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: ai_usage_limit_changes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_limit_changes (
    id uuid NOT NULL,
    scope_kind text NOT NULL,
    scope_id uuid NOT NULL,
    workspace_id uuid,
    project_id uuid,
    old_limit_micros bigint NOT NULL,
    new_limit_micros bigint NOT NULL,
    changed_by_user_id uuid NOT NULL,
    changed_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_usage_limit_changes_check CHECK ((((scope_kind = 'platform'::text) AND (workspace_id IS NULL) AND (project_id IS NULL)) OR ((scope_kind = 'workspace'::text) AND (workspace_id = scope_id) AND (project_id IS NULL)) OR ((scope_kind = 'project'::text) AND (workspace_id IS NOT NULL) AND (project_id = scope_id)))),
    CONSTRAINT ai_usage_limit_changes_new_limit_micros_check CHECK ((new_limit_micros >= 0)),
    CONSTRAINT ai_usage_limit_changes_old_limit_micros_check CHECK ((old_limit_micros >= 0)),
    CONSTRAINT ai_usage_limit_changes_scope_kind_check CHECK ((scope_kind = ANY (ARRAY['platform'::text, 'workspace'::text, 'project'::text])))
);


--
-- Name: ai_usage_limits; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_limits (
    scope_kind text NOT NULL,
    scope_id uuid NOT NULL,
    limit_micros bigint NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_usage_limits_check CHECK ((((scope_kind = 'platform'::text) AND (scope_id = '00000000-0000-0000-0000-000000000001'::uuid)) OR (scope_kind <> 'platform'::text))),
    CONSTRAINT ai_usage_limits_limit_micros_check CHECK ((limit_micros >= 0)),
    CONSTRAINT ai_usage_limits_scope_kind_check CHECK ((scope_kind = ANY (ARRAY['platform'::text, 'workspace'::text, 'project'::text])))
);


--
-- Name: ai_usage_period_buckets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_period_buckets (
    scope_kind text NOT NULL,
    scope_id uuid NOT NULL,
    workspace_id uuid,
    project_id uuid,
    run_budget_id uuid,
    period_start date NOT NULL,
    limit_micros bigint NOT NULL,
    spent_micros bigint DEFAULT 0 NOT NULL,
    reserved_micros bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_usage_period_buckets_check CHECK (((scope_kind <> 'run'::text) OR (period_start = '0001-01-01'::date))),
    CONSTRAINT ai_usage_period_buckets_check1 CHECK ((((scope_kind = 'platform'::text) AND (workspace_id IS NULL) AND (project_id IS NULL) AND (run_budget_id IS NULL)) OR ((scope_kind = 'workspace'::text) AND (workspace_id = scope_id) AND (project_id IS NULL) AND (run_budget_id IS NULL)) OR ((scope_kind = 'project'::text) AND (project_id = scope_id) AND (workspace_id IS NOT NULL) AND (run_budget_id IS NULL)) OR ((scope_kind = 'run'::text) AND (run_budget_id = scope_id) AND (workspace_id IS NOT NULL)))),
    CONSTRAINT ai_usage_period_buckets_limit_micros_check CHECK ((limit_micros >= 0)),
    CONSTRAINT ai_usage_period_buckets_reserved_micros_check CHECK ((reserved_micros >= 0)),
    CONSTRAINT ai_usage_period_buckets_scope_kind_check CHECK ((scope_kind = ANY (ARRAY['platform'::text, 'workspace'::text, 'project'::text, 'run'::text]))),
    CONSTRAINT ai_usage_period_buckets_spent_micros_check CHECK ((spent_micros >= 0))
);


--
-- Name: ai_usage_reservations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_reservations (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    project_id uuid,
    principal_kind text,
    principal_id text,
    run_id uuid NOT NULL,
    run_budget_id uuid NOT NULL,
    step_id text,
    job_id uuid,
    attempt_fence bigint,
    profile text NOT NULL,
    provider text NOT NULL,
    model text NOT NULL,
    purpose text NOT NULL,
    input_token_bound bigint NOT NULL,
    max_output_tokens bigint NOT NULL,
    max_reasoning_tokens bigint DEFAULT 0 NOT NULL,
    input_micros_per_million_tokens bigint NOT NULL,
    output_micros_per_million_tokens bigint NOT NULL,
    reasoning_micros_per_million_tokens bigint,
    price_version text NOT NULL,
    request_fingerprint text NOT NULL,
    reserved_cost_micros bigint NOT NULL,
    period_start date NOT NULL,
    state text DEFAULT 'open'::text NOT NULL,
    observed_input_tokens bigint,
    observed_output_tokens bigint,
    observed_reasoning_tokens bigint,
    observed_commercial_units bigint,
    recognized_cost_micros bigint DEFAULT 0 NOT NULL,
    actual_cost_micros bigint,
    overage_micros bigint DEFAULT 0 NOT NULL,
    current_price_version text,
    pricing_decision text,
    unresolved_reason text,
    void_reason text,
    provider_request_id text,
    provider_response_id text,
    resolved_by_user_id uuid,
    resolution_reason text,
    opened_at timestamp with time zone DEFAULT now() NOT NULL,
    settled_at timestamp with time zone,
    CONSTRAINT ai_usage_reservations_actual_cost_micros_check CHECK (((actual_cost_micros IS NULL) OR (actual_cost_micros >= 0))),
    CONSTRAINT ai_usage_reservations_attempt_fence_check CHECK (((attempt_fence IS NULL) OR (attempt_fence > 0))),
    CONSTRAINT ai_usage_reservations_check CHECK ((((principal_kind IS NULL) = (principal_id IS NULL)) AND ((principal_kind IS NULL) OR (principal_kind = ANY (ARRAY['user'::text, 'service'::text]))))),
    CONSTRAINT ai_usage_reservations_check1 CHECK (((job_id IS NULL) = (attempt_fence IS NULL))),
    CONSTRAINT ai_usage_reservations_check2 CHECK (((state <> 'settled'::text) OR ((actual_cost_micros IS NOT NULL) AND (settled_at IS NOT NULL)))),
    CONSTRAINT ai_usage_reservations_check3 CHECK (((state <> 'voided'::text) OR (void_reason IS NOT NULL))),
    CONSTRAINT ai_usage_reservations_input_micros_per_million_tokens_check CHECK ((input_micros_per_million_tokens >= 0)),
    CONSTRAINT ai_usage_reservations_input_token_bound_check CHECK ((input_token_bound >= 0)),
    CONSTRAINT ai_usage_reservations_max_output_tokens_check CHECK ((max_output_tokens >= 0)),
    CONSTRAINT ai_usage_reservations_max_reasoning_tokens_check CHECK ((max_reasoning_tokens >= 0)),
    CONSTRAINT ai_usage_reservations_observed_commercial_units_check CHECK (((observed_commercial_units IS NULL) OR (observed_commercial_units >= 0))),
    CONSTRAINT ai_usage_reservations_observed_input_tokens_check CHECK (((observed_input_tokens IS NULL) OR (observed_input_tokens >= 0))),
    CONSTRAINT ai_usage_reservations_observed_output_tokens_check CHECK (((observed_output_tokens IS NULL) OR (observed_output_tokens >= 0))),
    CONSTRAINT ai_usage_reservations_observed_reasoning_tokens_check CHECK (((observed_reasoning_tokens IS NULL) OR (observed_reasoning_tokens >= 0))),
    CONSTRAINT ai_usage_reservations_output_micros_per_million_tokens_check CHECK ((output_micros_per_million_tokens >= 0)),
    CONSTRAINT ai_usage_reservations_overage_micros_check CHECK ((overage_micros >= 0)),
    CONSTRAINT ai_usage_reservations_price_version_check CHECK (((length(price_version) >= 1) AND (length(price_version) <= 128))),
    CONSTRAINT ai_usage_reservations_purpose_check CHECK ((purpose = ANY (ARRAY['structured'::text, 'chat'::text, 'assistant'::text]))),
    CONSTRAINT ai_usage_reservations_reasoning_micros_per_million_tokens_check CHECK (((reasoning_micros_per_million_tokens IS NULL) OR (reasoning_micros_per_million_tokens >= 0))),
    CONSTRAINT ai_usage_reservations_recognized_cost_micros_check CHECK ((recognized_cost_micros >= 0)),
    CONSTRAINT ai_usage_reservations_request_fingerprint_check CHECK ((length(request_fingerprint) = 64)),
    CONSTRAINT ai_usage_reservations_reserved_cost_micros_check CHECK ((reserved_cost_micros >= 0)),
    CONSTRAINT ai_usage_reservations_state_check CHECK ((state = ANY (ARRAY['open'::text, 'observed'::text, 'unresolved'::text, 'settled'::text, 'voided'::text]))),
    CONSTRAINT ai_usage_reservations_step_id_check CHECK (((step_id IS NULL) OR ((length(step_id) >= 1) AND (length(step_id) <= 200))))
);


--
-- Name: ai_usage_rollout_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_rollout_state (
    singleton boolean DEFAULT true NOT NULL,
    ai_hard_caps_enforced boolean DEFAULT false NOT NULL,
    last_legacy_ledger_insert_at timestamp with time zone,
    hard_caps_enabled_at timestamp with time zone,
    hard_caps_disabled_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    ai_usage_export_activated boolean DEFAULT false NOT NULL,
    ai_usage_export_activated_at timestamp with time zone,
    ai_usage_export_deactivated_at timestamp with time zone,
    CONSTRAINT ai_usage_rollout_state_singleton_check CHECK (singleton)
);


--
-- Name: COLUMN ai_usage_rollout_state.ai_usage_export_activated; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.ai_usage_rollout_state.ai_usage_export_activated IS 'Durable gate for scheduling ai_usage_export_replay jobs. Set only by `deepref-server ai-usage enable-export`, cleared by `disable-export` before any rollback.';


--
-- Name: ai_usage_workspace_admission_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_workspace_admission_state (
    workspace_id uuid NOT NULL,
    blocked boolean DEFAULT false NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: ai_usage_workspace_blocks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage_workspace_blocks (
    workspace_id uuid NOT NULL,
    overage_micros bigint NOT NULL,
    reservation_id uuid NOT NULL,
    blocked_at timestamp with time zone DEFAULT now() NOT NULL,
    cleared_at timestamp with time zone,
    cleared_by_user_id uuid,
    clear_reason text,
    CONSTRAINT ai_usage_workspace_blocks_check CHECK (((cleared_at IS NULL) = (cleared_by_user_id IS NULL))),
    CONSTRAINT ai_usage_workspace_blocks_check1 CHECK (((cleared_at IS NULL) = (clear_reason IS NULL))),
    CONSTRAINT ai_usage_workspace_blocks_overage_micros_check CHECK ((overage_micros > 0))
);


--
-- Name: api_keys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.api_keys (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    created_by_user_id uuid NOT NULL,
    name text NOT NULL,
    prefix text NOT NULL,
    secret_hash bytea NOT NULL,
    scopes text[] NOT NULL,
    expires_at timestamp with time zone,
    last_used_at timestamp with time zone,
    revoked_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT api_keys_check CHECK (((expires_at IS NULL) OR (expires_at > created_at))),
    CONSTRAINT api_keys_name_check CHECK (((length(btrim(name)) >= 1) AND (length(btrim(name)) <= 100))),
    CONSTRAINT api_keys_prefix_check CHECK ((prefix = 'dr_key_v1_'::text)),
    CONSTRAINT api_keys_scopes_check CHECK ((((cardinality(scopes) >= 1) AND (cardinality(scopes) <= 2)) AND (array_position(scopes, NULL::text) IS NULL) AND (scopes <@ ARRAY['workspace:view'::text, 'project:view'::text]))),
    CONSTRAINT api_keys_secret_hash_check CHECK ((length(secret_hash) = 32))
);


--
-- Name: COLUMN api_keys.secret_hash; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.api_keys.secret_hash IS 'HMAC-SHA256 keyed by a 256-bit random secret over the fixed deepref-api-key-v1 context; constant-time verification. Never the secret.';


--
-- Name: appraisal_assessment_evidence; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.appraisal_assessment_evidence (
    id uuid NOT NULL,
    assessment_id uuid NOT NULL,
    project_id uuid NOT NULL,
    report_id uuid NOT NULL,
    question_id text NOT NULL,
    document_id uuid NOT NULL,
    block_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT appraisal_assessment_evidence_question_id_check CHECK ((length(btrim(question_id)) > 0))
);


--
-- Name: appraisal_assessments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.appraisal_assessments (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    report_id uuid NOT NULL,
    definition_id text NOT NULL,
    definition_version integer NOT NULL,
    responses jsonb NOT NULL,
    judgments jsonb NOT NULL,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    completed_at timestamp with time zone DEFAULT now() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT appraisal_assessments_actor_id_check CHECK ((length(btrim(actor_id)) > 0)),
    CONSTRAINT appraisal_assessments_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT appraisal_assessments_definition_id_check CHECK (((length(btrim(definition_id)) >= 1) AND (length(btrim(definition_id)) <= 100))),
    CONSTRAINT appraisal_assessments_definition_version_check CHECK ((definition_version > 0)),
    CONSTRAINT appraisal_assessments_judgments_check CHECK ((jsonb_typeof(judgments) = 'object'::text)),
    CONSTRAINT appraisal_assessments_responses_check CHECK ((jsonb_typeof(responses) = 'object'::text))
);


--
-- Name: appraisal_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.appraisal_events (
    id uuid NOT NULL,
    assessment_id uuid NOT NULL,
    project_id uuid NOT NULL,
    report_id uuid NOT NULL,
    event_type text NOT NULL,
    payload jsonb NOT NULL,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT appraisal_events_actor_id_check CHECK ((length(btrim(actor_id)) > 0)),
    CONSTRAINT appraisal_events_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT appraisal_events_event_type_check CHECK ((event_type = 'appraisal_completed'::text)),
    CONSTRAINT appraisal_events_payload_check CHECK ((jsonb_typeof(payload) = 'object'::text))
);


--
-- Name: assistant_agent_runs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.assistant_agent_runs (
    id uuid NOT NULL,
    conversation_id uuid NOT NULL,
    project_id uuid NOT NULL,
    trigger_message_id uuid NOT NULL,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    status text NOT NULL,
    model_route jsonb NOT NULL,
    semantic_contract_id text,
    runtime_version text NOT NULL,
    build_provenance jsonb DEFAULT '{}'::jsonb NOT NULL,
    rig_run jsonb,
    effect_log jsonb,
    run_spec_hash text,
    answer_message_id uuid,
    plan_id uuid,
    error jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone,
    principal_user_id uuid,
    authz_principal text,
    CONSTRAINT assistant_agent_runs_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'suspended'::text, 'completed'::text, 'failed'::text, 'cancelled'::text]))),
    CONSTRAINT assistant_agent_runs_status_no_suspended CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'completed'::text, 'failed'::text, 'cancelled'::text])))
);


--
-- Name: assistant_conversations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.assistant_conversations (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    title text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT assistant_conversations_title_check CHECK (((length(btrim(title)) >= 1) AND (length(btrim(title)) <= 500)))
);


--
-- Name: assistant_messages; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.assistant_messages (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    conversation_id uuid NOT NULL,
    role text NOT NULL,
    content text NOT NULL,
    tool_calls jsonb,
    tool_results jsonb,
    metadata jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT assistant_messages_metadata_check CHECK (((metadata IS NULL) OR (jsonb_typeof(metadata) = 'object'::text))),
    CONSTRAINT assistant_messages_role_check CHECK ((role = ANY (ARRAY['user'::text, 'assistant'::text, 'system'::text, 'tool'::text]))),
    CONSTRAINT assistant_messages_tool_calls_check CHECK (((tool_calls IS NULL) OR (jsonb_typeof(tool_calls) = 'array'::text))),
    CONSTRAINT assistant_messages_tool_results_check CHECK (((tool_results IS NULL) OR (jsonb_typeof(tool_results) = ANY (ARRAY['array'::text, 'object'::text]))))
);


--
-- Name: assistant_plans; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.assistant_plans (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    conversation_id uuid NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    summary text NOT NULL,
    actions jsonb NOT NULL,
    results jsonb,
    error text,
    created_by_kind text NOT NULL,
    created_by_id text NOT NULL,
    model text NOT NULL,
    prompt_version text NOT NULL,
    evidence jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    resolved_by_kind text,
    resolved_by_id text,
    resolved_at timestamp with time zone,
    CONSTRAINT assistant_plans_actions_check CHECK ((jsonb_typeof(actions) = 'array'::text)),
    CONSTRAINT assistant_plans_evidence_check CHECK ((jsonb_typeof(evidence) = 'array'::text)),
    CONSTRAINT assistant_plans_results_check CHECK (((results IS NULL) OR (jsonb_typeof(results) = 'array'::text))),
    CONSTRAINT assistant_plans_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'confirmed'::text, 'rejected'::text, 'executed'::text, 'failed'::text])))
);


--
-- Name: assistant_run_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.assistant_run_events (
    run_id uuid NOT NULL,
    seq bigint NOT NULL,
    kind text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT assistant_run_events_kind_check CHECK ((kind = ANY (ARRAY['status'::text, 'text'::text, 'replace'::text, 'tool_start'::text, 'tool_complete'::text, 'plan'::text, 'done'::text, 'error'::text]))),
    CONSTRAINT assistant_run_events_seq_check CHECK ((seq >= 0))
);


--
-- Name: automation_definition_steps; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.automation_definition_steps (
    project_id uuid NOT NULL,
    definition_id uuid NOT NULL,
    ordinal integer NOT NULL,
    step_key text NOT NULL,
    step_kind text NOT NULL,
    CONSTRAINT automation_definition_steps_ordinal_check CHECK (((ordinal >= 0) AND (ordinal <= 99))),
    CONSTRAINT automation_definition_steps_step_key_check CHECK (((length(btrim(step_key)) >= 1) AND (length(btrim(step_key)) <= 200))),
    CONSTRAINT automation_definition_steps_step_kind_check CHECK ((step_kind = ANY (ARRAY['deterministic_action'::text, 'ai_task'::text, 'agent'::text, 'notification'::text, 'domain_command'::text, 'domain_proposal'::text])))
);


--
-- Name: automation_definitions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.automation_definitions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    name text NOT NULL,
    trigger_kind text NOT NULL,
    recipe_id text NOT NULL,
    recipe_version integer NOT NULL,
    status text NOT NULL,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT automation_definitions_actor_id_check CHECK (((length(btrim(actor_id)) >= 1) AND (length(btrim(actor_id)) <= 200))),
    CONSTRAINT automation_definitions_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT automation_definitions_name_check CHECK (((length(btrim(name)) >= 1) AND (length(btrim(name)) <= 200))),
    CONSTRAINT automation_definitions_recipe_id_check CHECK ((recipe_id = ANY (ARRAY['project_maintenance'::text, 'review_screening'::text, 'review_duplicate_detection'::text, 'review_study_classification'::text, 'review_study_grouping'::text, 'review_appraisal_prefill'::text, 'review_data_extraction'::text]))),
    CONSTRAINT automation_definitions_recipe_version_check CHECK ((recipe_version = 1)),
    CONSTRAINT automation_definitions_status_check CHECK ((status = ANY (ARRAY['active'::text, 'paused'::text]))),
    CONSTRAINT automation_definitions_trigger_kind_check CHECK ((trigger_kind = ANY (ARRAY['report_added'::text, 'acquisition_completed'::text, 'full_text_attached'::text, 'report_included'::text, 'study_created'::text, 'appraisal_completed'::text, 'manual'::text])))
);


--
-- Name: automation_run_details; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.automation_run_details (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    definition_id uuid NOT NULL,
    recipe_id text NOT NULL,
    recipe_version integer NOT NULL,
    trigger_kind text NOT NULL,
    trigger_reference text,
    idempotency_key text NOT NULL
);


--
-- Name: automation_step_details; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.automation_step_details (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    run_id uuid NOT NULL,
    ordinal integer NOT NULL,
    step_key text NOT NULL,
    step_kind text NOT NULL,
    accepted_attempt_id uuid,
    input_fingerprint text
);


--
-- Name: billing_accounts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.billing_accounts (
    id uuid NOT NULL,
    workspace_id uuid,
    provider_scope text NOT NULL,
    operation_id uuid NOT NULL,
    customer_ref text,
    provisioning text NOT NULL,
    subscription_ref text,
    product_ref text,
    generation bigint DEFAULT 0 NOT NULL,
    projection jsonb DEFAULT '{"kind": "unknown"}'::jsonb NOT NULL,
    review_required boolean DEFAULT false NOT NULL,
    review_id uuid,
    resolved_review_id uuid,
    reconcile_after timestamp with time zone DEFAULT now() NOT NULL,
    reconciled_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    detached_at timestamp with time zone,
    purge_after timestamp with time zone,
    closure_workspace_id uuid,
    closure_customer_ref text,
    closure_subscription_ref text,
    closure_captured_at timestamp with time zone,
    CONSTRAINT billing_accounts_check CHECK (((subscription_ref IS NULL) = (product_ref IS NULL))),
    CONSTRAINT billing_accounts_check1 CHECK (((provisioning = 'mapped'::text) = (customer_ref IS NOT NULL))),
    CONSTRAINT billing_accounts_check2 CHECK (((provisioning = 'detached'::text) = (detached_at IS NOT NULL))),
    CONSTRAINT billing_accounts_closure_customer_ref_check CHECK (((length(closure_customer_ref) >= 1) AND (length(closure_customer_ref) <= 128))),
    CONSTRAINT billing_accounts_closure_subscription_ref_check CHECK (((length(closure_subscription_ref) >= 1) AND (length(closure_subscription_ref) <= 128))),
    CONSTRAINT billing_accounts_customer_ref_check CHECK (((length(customer_ref) >= 1) AND (length(customer_ref) <= 128))),
    CONSTRAINT billing_accounts_generation_check CHECK ((generation >= 0)),
    CONSTRAINT billing_accounts_product_ref_check CHECK (((length(product_ref) >= 1) AND (length(product_ref) <= 128))),
    CONSTRAINT billing_accounts_provider_scope_check CHECK (((length(provider_scope) >= 1) AND (length(provider_scope) <= 256))),
    CONSTRAINT billing_accounts_provisioning_check CHECK ((provisioning = ANY (ARRAY['ambiguous'::text, 'mapped'::text, 'detached'::text]))),
    CONSTRAINT billing_accounts_subscription_ref_check CHECK (((length(subscription_ref) >= 1) AND (length(subscription_ref) <= 128))),
    CONSTRAINT billing_closure_context_shape CHECK ((((closure_captured_at IS NULL) AND (closure_workspace_id IS NULL) AND (closure_customer_ref IS NULL) AND (closure_subscription_ref IS NULL)) OR ((closure_captured_at IS NOT NULL) AND (provisioning = 'detached'::text))))
);


--
-- Name: COLUMN billing_accounts.closure_workspace_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.billing_accounts.closure_workspace_id IS 'Restricted historical billing tenant, deliberately without a live workspace FK. Not authority to restore a tenant or evidence of provider closure.';


--
-- Name: COLUMN billing_accounts.closure_captured_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.billing_accounts.closure_captured_at IS 'Actual original-context capture during detachment; NULL on unknown historical tombstones. Does not change purge_after or prove provider closure.';


--
-- Name: billing_free_enrollments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.billing_free_enrollments (
    workspace_id uuid NOT NULL,
    operation_id uuid NOT NULL,
    entitlements jsonb NOT NULL,
    enrolled_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: billing_inbox_invalidations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.billing_inbox_invalidations (
    inbox_id bigint NOT NULL,
    account_id uuid NOT NULL
);


--
-- Name: billing_usage_fact_targets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.billing_usage_fact_targets (
    fact_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    provider_scope text NOT NULL,
    customer_ref text NOT NULL,
    account_generation bigint NOT NULL,
    bound_at timestamp with time zone DEFAULT now() NOT NULL,
    account_id uuid,
    mode text,
    business_id text,
    event_name text,
    original_timestamp timestamp with time zone,
    metadata jsonb,
    wire_digest text,
    dispatch_claimed_at timestamp with time zone,
    dispatch_state text DEFAULT 'bound'::text NOT NULL,
    CONSTRAINT billing_usage_fact_targets_dispatch_state_check CHECK ((dispatch_state = ANY (ARRAY['bound'::text, 'dispatched'::text, 'quarantined'::text, 'duplicate'::text, 'delivered'::text])))
);


--
-- Name: billing_usage_resolutions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.billing_usage_resolutions (
    fact_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    resolution_id uuid NOT NULL,
    resolved_by uuid NOT NULL,
    action text NOT NULL,
    reason text NOT NULL,
    notes text,
    resolved_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT billing_usage_resolutions_action_check CHECK ((action = ANY (ARRAY['manual_reconciliation'::text, 'reopened_retry'::text, 'refused'::text]))),
    CONSTRAINT billing_usage_resolutions_reason_check CHECK ((reason = ANY (ARRAY['external_adjustment_confirmed'::text, 'provider_duplicate_verified'::text, 'manual_credit_issued'::text, 'rejected_unrecoverable'::text])))
);


--
-- Name: citations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.citations (
    project_id uuid NOT NULL,
    source_report_id uuid NOT NULL,
    target_report_id uuid NOT NULL,
    source text DEFAULT 'legacy-citation'::text NOT NULL,
    acquisition_run_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: crossref_raw_records; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.crossref_raw_records (
    canonical_doi text NOT NULL,
    raw jsonb NOT NULL,
    fetched_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: database_role_contract; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.database_role_contract (
    singleton boolean DEFAULT true NOT NULL,
    activated_at timestamp with time zone,
    activated_by name,
    CONSTRAINT database_role_contract_singleton_check CHECK (singleton)
);


--
-- Name: dedupe_proposals; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dedupe_proposals (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    record_id uuid NOT NULL,
    candidate_report_id uuid,
    proposal_kind text NOT NULL,
    title_similarity double precision DEFAULT 0 NOT NULL,
    year_match boolean,
    first_author_similarity double precision,
    exact_identifier_match boolean DEFAULT false NOT NULL,
    conflicting_identifier boolean DEFAULT false NOT NULL,
    score double precision DEFAULT 0 NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    reviewer_kind text,
    reviewer_id text,
    decided_at timestamp with time zone,
    decision_reason text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT dedupe_proposals_first_author_similarity_check CHECK (((first_author_similarity IS NULL) OR ((first_author_similarity >= (0)::double precision) AND (first_author_similarity <= (1)::double precision)))),
    CONSTRAINT dedupe_proposals_proposal_kind_check CHECK ((proposal_kind = ANY (ARRAY['fuzzy'::text, 'conflict'::text]))),
    CONSTRAINT dedupe_proposals_reviewer_id_check CHECK (((btrim(COALESCE(reviewer_id, ''::text)) = ''::text) OR (btrim(reviewer_id) <> ''::text))),
    CONSTRAINT dedupe_proposals_reviewer_kind_check CHECK (((btrim(COALESCE(reviewer_kind, ''::text)) = ''::text) OR (reviewer_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text])))),
    CONSTRAINT dedupe_proposals_revision_check CHECK ((revision >= 0)),
    CONSTRAINT dedupe_proposals_score_check CHECK (((score >= (0)::double precision) AND (score <= (1)::double precision))),
    CONSTRAINT dedupe_proposals_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'accepted'::text, 'rejected'::text]))),
    CONSTRAINT dedupe_proposals_status_reviewer_check CHECK ((((status = 'pending'::text) AND (reviewer_kind IS NULL) AND (reviewer_id IS NULL) AND (decided_at IS NULL) AND (decision_reason IS NULL)) OR ((status = ANY (ARRAY['accepted'::text, 'rejected'::text])) AND (btrim(COALESCE(reviewer_kind, ''::text)) <> ''::text) AND (btrim(COALESCE(reviewer_id, ''::text)) <> ''::text) AND (decided_at IS NOT NULL) AND (btrim(COALESCE(decision_reason, ''::text)) <> ''::text)))),
    CONSTRAINT dedupe_proposals_title_similarity_check CHECK (((title_similarity >= (0)::double precision) AND (title_similarity <= (1)::double precision)))
);


--
-- Name: dedupe_resolution_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dedupe_resolution_events (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    record_id uuid NOT NULL,
    prior_report_id uuid,
    resolved_report_id uuid,
    action text NOT NULL,
    reason text NOT NULL,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    proposal_id uuid,
    reverted_event_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT dedupe_resolution_events_action_check CHECK ((action = ANY (ARRAY['auto_link'::text, 'create_report'::text, 'accept_proposal'::text, 'reject_proposal'::text, 'create_new'::text, 'link'::text, 'reassign'::text, 'revert'::text]))),
    CONSTRAINT dedupe_resolution_events_actor_id_check CHECK ((btrim(actor_id) <> ''::text)),
    CONSTRAINT dedupe_resolution_events_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT dedupe_resolution_events_reason_check CHECK ((btrim(reason) <> ''::text))
);


--
-- Name: document_block_embeddings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.document_block_embeddings (
    document_block_id uuid NOT NULL,
    model_identifier text NOT NULL,
    generation text NOT NULL,
    content_hash text NOT NULL,
    dimension integer NOT NULL,
    embedding public.vector NOT NULL,
    is_current boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT document_block_embeddings_check CHECK ((public.vector_dims(embedding) = dimension)),
    CONSTRAINT document_block_embeddings_content_hash_check CHECK ((content_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT document_block_embeddings_dimension_check CHECK ((dimension > 0)),
    CONSTRAINT document_block_embeddings_generation_check CHECK ((length(btrim(generation)) > 0)),
    CONSTRAINT document_block_embeddings_model_identifier_check CHECK ((length(btrim(model_identifier)) > 0))
);


--
-- Name: document_blocks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.document_blocks (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    parser_version text NOT NULL,
    page_number integer NOT NULL,
    kind text NOT NULL,
    section_path text[] DEFAULT '{}'::text[] NOT NULL,
    ordinal integer NOT NULL,
    text text NOT NULL,
    bbox jsonb,
    search_vector tsvector GENERATED ALWAYS AS (to_tsvector('simple'::regconfig, text)) STORED,
    content_hash text NOT NULL,
    active boolean DEFAULT false NOT NULL,
    page_width double precision,
    page_height double precision,
    CONSTRAINT document_blocks_bbox_check CHECK (((bbox IS NULL) OR ((jsonb_typeof(bbox) = 'object'::text) AND (jsonb_typeof((bbox -> 'x'::text)) = 'number'::text) AND (jsonb_typeof((bbox -> 'y'::text)) = 'number'::text) AND (jsonb_typeof((bbox -> 'width'::text)) = 'number'::text) AND (jsonb_typeof((bbox -> 'height'::text)) = 'number'::text) AND ((((bbox ->> 'x'::text))::double precision >= (0)::double precision) AND (((bbox ->> 'x'::text))::double precision <= (1)::double precision)) AND ((((bbox ->> 'y'::text))::double precision >= (0)::double precision) AND (((bbox ->> 'y'::text))::double precision <= (1)::double precision)) AND (((bbox ->> 'width'::text))::double precision > (0)::double precision) AND (((bbox ->> 'width'::text))::double precision <= (1)::double precision) AND (((bbox ->> 'height'::text))::double precision > (0)::double precision) AND (((bbox ->> 'height'::text))::double precision <= (1)::double precision) AND ((((bbox ->> 'x'::text))::double precision + ((bbox ->> 'width'::text))::double precision) <= (1)::double precision) AND ((((bbox ->> 'y'::text))::double precision + ((bbox ->> 'height'::text))::double precision) <= (1)::double precision)))),
    CONSTRAINT document_blocks_content_hash_check CHECK ((content_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT document_blocks_kind_check CHECK ((kind = ANY (ARRAY['text'::text, 'heading'::text, 'table'::text, 'figure_caption'::text, 'caption'::text, 'reference'::text, 'title'::text, 'front_matter'::text]))),
    CONSTRAINT document_blocks_ordinal_check CHECK ((ordinal >= 0)),
    CONSTRAINT document_blocks_page_geometry_check CHECK ((((page_width IS NULL) AND (page_height IS NULL)) OR ((page_width > (0)::double precision) AND (page_height > (0)::double precision) AND (page_width <> ALL (ARRAY['Infinity'::double precision, '-Infinity'::double precision, 'NaN'::double precision])) AND (page_height <> ALL (ARRAY['Infinity'::double precision, '-Infinity'::double precision, 'NaN'::double precision]))))),
    CONSTRAINT document_blocks_page_number_check CHECK ((page_number > 0)),
    CONSTRAINT document_blocks_parser_version_check CHECK ((length(btrim(parser_version)) > 0)),
    CONSTRAINT document_blocks_text_check CHECK ((length(text) <= 1000000))
);


--
-- Name: document_pages; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.document_pages (
    document_id uuid NOT NULL,
    parser_version text NOT NULL,
    page_number integer NOT NULL,
    width double precision NOT NULL,
    height double precision NOT NULL,
    ocr_required boolean DEFAULT false NOT NULL,
    active boolean DEFAULT false NOT NULL,
    CONSTRAINT document_pages_check CHECK (((width > (0)::double precision) AND (height > (0)::double precision) AND (width <> ALL (ARRAY['Infinity'::double precision, '-Infinity'::double precision, 'NaN'::double precision])) AND (height <> ALL (ARRAY['Infinity'::double precision, '-Infinity'::double precision, 'NaN'::double precision])))),
    CONSTRAINT document_pages_page_number_check CHECK ((page_number > 0)),
    CONSTRAINT document_pages_parser_version_check CHECK ((length(btrim(parser_version)) > 0))
);


--
-- Name: document_references; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.document_references (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    parser_version text NOT NULL,
    ordinal integer NOT NULL,
    raw text NOT NULL,
    title text,
    authors jsonb DEFAULT '[]'::jsonb NOT NULL,
    year integer,
    venue text,
    doi text,
    source text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT document_references_ordinal_check CHECK ((ordinal > 0)),
    CONSTRAINT document_references_source_check CHECK ((source = ANY (ARRAY['native'::text, 'grobid'::text])))
);


--
-- Name: document_sections; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.document_sections (
    id uuid NOT NULL,
    document_id uuid NOT NULL,
    parser_version text NOT NULL,
    ordinal integer NOT NULL,
    number text,
    title text NOT NULL,
    depth integer NOT NULL,
    path text[] DEFAULT '{}'::text[] NOT NULL,
    source text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT document_sections_depth_check CHECK ((depth > 0)),
    CONSTRAINT document_sections_ordinal_check CHECK ((ordinal > 0)),
    CONSTRAINT document_sections_source_check CHECK ((source = ANY (ARRAY['native'::text, 'grobid'::text])))
);


--
-- Name: documents; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.documents (
    id uuid NOT NULL,
    report_id uuid NOT NULL,
    object_key text,
    content_hash text,
    mime_type text NOT NULL,
    byte_size bigint NOT NULL,
    parser_version text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    project_id uuid NOT NULL,
    source text DEFAULT 'upload'::text NOT NULL,
    status text DEFAULT 'uploaded'::text NOT NULL,
    original_filename text,
    external_url text,
    parser_error text,
    active_parser_version text,
    ocr_required boolean DEFAULT false NOT NULL,
    actor_kind text DEFAULT 'system'::text NOT NULL,
    actor_id text DEFAULT 'legacy-migration'::text NOT NULL,
    content_available_at timestamp with time zone,
    parsed_at timestamp with time zone,
    failed_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    reparsed_by_kind text,
    reparsed_by_id text,
    reparsed_at timestamp with time zone,
    identity_check jsonb,
    identity_acknowledged_at timestamp with time zone,
    CONSTRAINT documents_actor_id_check CHECK ((length(btrim(actor_id)) > 0)),
    CONSTRAINT documents_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT documents_byte_size_check CHECK ((byte_size >= 0)),
    CONSTRAINT documents_content_hash_check CHECK (((content_hash IS NULL) OR (content_hash ~ '^[0-9a-f]{64}$'::text))),
    CONSTRAINT documents_content_shape_check CHECK ((((status = ANY (ARRAY['missing'::text, 'external'::text, 'retrieving'::text])) AND (object_key IS NULL) AND (content_hash IS NULL) AND (byte_size = 0)) OR ((status = ANY (ARRAY['uploaded'::text, 'available'::text])) AND (object_key IS NOT NULL) AND (content_hash IS NOT NULL) AND (byte_size > 0) AND (mime_type = 'application/pdf'::text)) OR ((status = 'failed'::text) AND (((object_key IS NULL) AND (content_hash IS NULL) AND (byte_size = 0)) OR ((object_key IS NOT NULL) AND (content_hash IS NOT NULL) AND (byte_size > 0) AND (mime_type = 'application/pdf'::text)))))),
    CONSTRAINT documents_external_url_check CHECK ((((source = 'external_url'::text) AND (external_url ~ '^https://[^[:space:]]+$'::text)) OR ((source <> 'external_url'::text) AND (external_url IS NULL)))),
    CONSTRAINT documents_filename_check CHECK (((original_filename IS NULL) OR ((length(original_filename) >= 1) AND (length(original_filename) <= 255)))),
    CONSTRAINT documents_object_key_check CHECK (((object_key IS NULL) OR (object_key ~ '^documents/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text))),
    CONSTRAINT documents_object_key_tenant_layout_check CHECK (((object_key IS NULL) OR (object_key ~ '^documents/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text) OR (object_key ~ '^workspaces/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/projects/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/documents/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text))),
    CONSTRAINT documents_parser_shape_check CHECK (((status <> 'available'::text) OR (active_parser_version IS NOT NULL))),
    CONSTRAINT documents_source_check CHECK ((source = ANY (ARRAY['upload'::text, 'external_url'::text, 'resolver'::text]))),
    CONSTRAINT documents_status_check CHECK ((status = ANY (ARRAY['missing'::text, 'external'::text, 'uploaded'::text, 'retrieving'::text, 'available'::text, 'failed'::text])))
);


--
-- Name: durable_attempts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.durable_attempts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    run_id uuid NOT NULL,
    step_id uuid NOT NULL,
    attempt_number integer NOT NULL,
    status text NOT NULL,
    worker_id text,
    job_id uuid,
    job_attempt integer,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone,
    error text,
    CONSTRAINT durable_attempts_attempt_number_check CHECK ((attempt_number > 0)),
    CONSTRAINT durable_attempts_check CHECK (((job_id IS NULL) = (job_attempt IS NULL))),
    CONSTRAINT durable_attempts_status_check CHECK ((status = ANY (ARRAY['running'::text, 'waiting'::text, 'completed'::text, 'failed'::text, 'blocked'::text, 'cancelled'::text, 'skipped'::text])))
);


--
-- Name: durable_runs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.durable_runs (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    kind text NOT NULL,
    job_id uuid,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    idempotency_key text NOT NULL,
    status text NOT NULL,
    error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    CONSTRAINT durable_runs_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT durable_runs_kind_check CHECK ((kind = ANY (ARRAY['automation'::text, 'workflow'::text]))),
    CONSTRAINT durable_runs_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'waiting'::text, 'blocked'::text, 'completed'::text, 'failed'::text, 'cancelled'::text])))
);


--
-- Name: durable_steps; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.durable_steps (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    run_id uuid NOT NULL,
    handler_kind text NOT NULL,
    step_key text NOT NULL,
    ordinal integer,
    status text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    claimed_by text,
    claimed_job_id uuid,
    claimed_job_attempt integer,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    error text,
    output jsonb,
    CONSTRAINT durable_steps_attempts_check CHECK ((attempts >= 0)),
    CONSTRAINT durable_steps_check CHECK (((claimed_job_id IS NULL) = (claimed_job_attempt IS NULL))),
    CONSTRAINT durable_steps_claimed_job_attempt_check CHECK (((claimed_job_attempt IS NULL) OR (claimed_job_attempt > 0))),
    CONSTRAINT durable_steps_handler_kind_check CHECK ((handler_kind = ANY (ARRAY['automation'::text, 'workflow'::text, 'review'::text]))),
    CONSTRAINT durable_steps_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'queued'::text, 'running'::text, 'waiting'::text, 'blocked'::text, 'completed'::text, 'failed'::text, 'skipped'::text, 'cancelled'::text])))
);


--
-- Name: eligibility_criteria; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.eligibility_criteria (
    id uuid NOT NULL,
    protocol_version_id uuid NOT NULL,
    criterion_type text NOT NULL,
    label text NOT NULL,
    description text NOT NULL,
    ordinal integer DEFAULT 0 NOT NULL,
    stage text DEFAULT 'both'::text NOT NULL,
    dimension text DEFAULT 'other'::text NOT NULL,
    CONSTRAINT eligibility_criteria_criterion_type_check CHECK ((criterion_type = ANY (ARRAY['include'::text, 'exclude'::text]))),
    CONSTRAINT eligibility_criteria_dimension_check CHECK ((dimension = ANY (ARRAY['population'::text, 'intervention'::text, 'comparator'::text, 'outcome'::text, 'design'::text, 'setting'::text, 'language'::text, 'date'::text, 'other'::text]))),
    CONSTRAINT eligibility_criteria_stage_check CHECK ((stage = ANY (ARRAY['title_abstract'::text, 'full_text'::text, 'both'::text])))
);


--
-- Name: exclusion_reasons; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.exclusion_reasons (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    code text NOT NULL,
    label text NOT NULL,
    stage text DEFAULT 'full_text'::text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT exclusion_reasons_stage_check CHECK ((stage = ANY (ARRAY['title_abstract'::text, 'full_text'::text])))
);


--
-- Name: extraction_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.extraction_events (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    study_id uuid NOT NULL,
    proposal_id uuid NOT NULL,
    event_type text NOT NULL,
    payload jsonb NOT NULL,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT extraction_events_actor_id_check CHECK ((length(btrim(actor_id)) > 0)),
    CONSTRAINT extraction_events_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT extraction_events_event_type_check CHECK ((event_type = 'extraction_values_approved'::text)),
    CONSTRAINT extraction_events_payload_check CHECK ((jsonb_typeof(payload) = 'object'::text))
);


--
-- Name: extraction_field_definitions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.extraction_field_definitions (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    version integer NOT NULL,
    field_key text NOT NULL,
    label text NOT NULL,
    value_type text NOT NULL,
    required boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT extraction_field_definitions_field_key_check CHECK (((length(btrim(field_key)) >= 1) AND (length(btrim(field_key)) <= 100))),
    CONSTRAINT extraction_field_definitions_label_check CHECK (((length(btrim(label)) >= 1) AND (length(btrim(label)) <= 200))),
    CONSTRAINT extraction_field_definitions_value_type_check CHECK ((value_type = ANY (ARRAY['text'::text, 'number'::text, 'boolean'::text, 'date'::text]))),
    CONSTRAINT extraction_field_definitions_version_check CHECK ((version > 0))
);


--
-- Name: extraction_values; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.extraction_values (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    study_id uuid NOT NULL,
    report_id uuid,
    field_definition_id uuid NOT NULL,
    field_definition_version integer NOT NULL,
    value_type text NOT NULL,
    text_value text,
    number_value double precision,
    boolean_value boolean,
    date_value date,
    rationale text,
    source_document_id uuid,
    source_block_id uuid,
    source_page integer,
    source_parser_version text,
    source_content_hash text,
    approved_by_actor_kind text NOT NULL,
    approved_by_actor_id text NOT NULL,
    approved_at timestamp with time zone DEFAULT now() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    superseded_at timestamp with time zone,
    superseded_by_actor_kind text,
    superseded_by_actor_id text,
    needs_verification boolean DEFAULT false NOT NULL,
    verified_at timestamp with time zone,
    verified_by_actor_kind text,
    verified_by_actor_id text,
    activity_id uuid,
    CONSTRAINT extraction_values_approved_by_actor_id_check CHECK ((length(btrim(approved_by_actor_id)) > 0)),
    CONSTRAINT extraction_values_approved_by_actor_kind_check CHECK ((approved_by_actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT extraction_values_check CHECK ((((value_type = 'text'::text) AND (text_value IS NOT NULL) AND (number_value IS NULL) AND (boolean_value IS NULL) AND (date_value IS NULL)) OR ((value_type = 'number'::text) AND (text_value IS NULL) AND (number_value IS NOT NULL) AND (number_value = number_value) AND (number_value < 'Infinity'::double precision) AND (number_value > '-Infinity'::double precision) AND ((number_value)::text <> ALL (ARRAY['NaN'::text, 'Infinity'::text, '-Infinity'::text])) AND (boolean_value IS NULL) AND (date_value IS NULL)) OR ((value_type = 'boolean'::text) AND (text_value IS NULL) AND (number_value IS NULL) AND (boolean_value IS NOT NULL) AND (date_value IS NULL)) OR ((value_type = 'date'::text) AND (text_value IS NULL) AND (number_value IS NULL) AND (boolean_value IS NULL) AND (date_value IS NOT NULL)))),
    CONSTRAINT extraction_values_field_definition_version_check CHECK ((field_definition_version > 0)),
    CONSTRAINT extraction_values_rationale_check CHECK ((length(btrim(rationale)) > 0)),
    CONSTRAINT extraction_values_source_all_or_none CHECK ((((report_id IS NULL) AND (source_document_id IS NULL) AND (source_block_id IS NULL) AND (source_page IS NULL) AND (source_parser_version IS NULL) AND (source_content_hash IS NULL)) OR ((report_id IS NOT NULL) AND (source_document_id IS NOT NULL) AND (source_block_id IS NOT NULL) AND (source_page IS NOT NULL) AND (source_parser_version IS NOT NULL) AND (source_content_hash IS NOT NULL)))),
    CONSTRAINT extraction_values_source_content_hash_check CHECK ((source_content_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT extraction_values_source_page_check CHECK ((source_page > 0)),
    CONSTRAINT extraction_values_source_parser_version_check CHECK ((length(btrim(source_parser_version)) > 0)),
    CONSTRAINT extraction_values_superseded_by_actor_id_check CHECK ((length(btrim(superseded_by_actor_id)) > 0)),
    CONSTRAINT extraction_values_superseded_by_actor_kind_check CHECK ((superseded_by_actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT extraction_values_supersession_actor CHECK ((((superseded_at IS NULL) AND (superseded_by_actor_kind IS NULL) AND (superseded_by_actor_id IS NULL)) OR ((superseded_at IS NOT NULL) AND (superseded_by_actor_kind IS NOT NULL) AND (superseded_by_actor_id IS NOT NULL)))),
    CONSTRAINT extraction_values_value_type_check CHECK ((value_type = ANY (ARRAY['text'::text, 'number'::text, 'boolean'::text, 'date'::text])))
);


--
-- Name: fetched_citation_facts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.fetched_citation_facts (
    source_doi text NOT NULL,
    target_doi text NOT NULL,
    source text DEFAULT 'crossref-reference'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: fetched_unresolved_reference_facts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.fetched_unresolved_reference_facts (
    id text NOT NULL,
    source_doi text NOT NULL,
    raw_unstructured text,
    article_title text,
    author text,
    year text,
    volume text,
    first_page text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: fulfilled_account_deletions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.fulfilled_account_deletions (
    user_id uuid NOT NULL,
    completed_at timestamp with time zone DEFAULT now() NOT NULL,
    email_hmac_sha256 text[] DEFAULT ARRAY[]::text[] NOT NULL,
    physical_verified_at timestamp with time zone,
    logical_scrubbed_at timestamp with time zone
);


--
-- Name: COLUMN fulfilled_account_deletions.email_hmac_sha256; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.fulfilled_account_deletions.email_hmac_sha256 IS 'Lowercase hex HMAC-SHA256 digests of normalized account addresses, used to scrub invitations after restore; keyed with DEEPREF_DELETION_DIGEST_KEY held outside PostgreSQL; never plaintext email.';


--
-- Name: COLUMN fulfilled_account_deletions.physical_verified_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.fulfilled_account_deletions.physical_verified_at IS 'Set only by new completion after every targeted object deletion and writer-owned staging cleanup acknowledgement. Historical NULL is unverified, not proof of remaining bytes.';


--
-- Name: integration_inbox_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.integration_inbox_events (
    id bigint NOT NULL,
    provider text NOT NULL,
    provider_event_id text NOT NULL,
    event_type text NOT NULL,
    payload jsonb,
    payload_expires_at timestamp with time zone,
    payload_purged_at timestamp with time zone,
    received_at timestamp with time zone DEFAULT now() NOT NULL,
    processing_state text DEFAULT 'received'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    max_attempts integer DEFAULT 8 NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    processed_at timestamp with time zone,
    quarantine_reason text,
    lease_owner text,
    lease_expires_at timestamp with time zone,
    claim_generation bigint DEFAULT 0 NOT NULL,
    CONSTRAINT integration_inbox_events_attempts_check CHECK ((attempts >= 0)),
    CONSTRAINT integration_inbox_events_check CHECK (((lease_owner IS NULL) = (lease_expires_at IS NULL))),
    CONSTRAINT integration_inbox_events_check1 CHECK (((processing_state = 'processed'::text) = (processed_at IS NOT NULL))),
    CONSTRAINT integration_inbox_events_check2 CHECK (((payload IS NOT NULL) OR (processing_state = 'processed'::text))),
    CONSTRAINT integration_inbox_events_check3 CHECK (((payload_expires_at IS NULL) OR (processing_state = 'processed'::text))),
    CONSTRAINT integration_inbox_events_claim_generation_check CHECK ((claim_generation >= 0)),
    CONSTRAINT integration_inbox_events_event_type_check CHECK (((event_type <> ''::text) AND (event_type = btrim(event_type)))),
    CONSTRAINT integration_inbox_events_max_attempts_check CHECK ((max_attempts > 0)),
    CONSTRAINT integration_inbox_events_processing_state_check CHECK ((processing_state = ANY (ARRAY['received'::text, 'processing'::text, 'processed'::text, 'failed'::text, 'quarantined'::text]))),
    CONSTRAINT integration_inbox_events_provider_event_id_check CHECK (((char_length(provider_event_id) >= 1) AND (char_length(provider_event_id) <= 255)))
);


--
-- Name: integration_inbox_events_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.integration_inbox_events ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.integration_inbox_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);


--
-- Name: jobs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.jobs (
    id uuid NOT NULL,
    kind text NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    state text DEFAULT 'queued'::text NOT NULL,
    priority integer DEFAULT 0 NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    max_attempts integer DEFAULT 5 NOT NULL,
    available_at timestamp with time zone DEFAULT now() NOT NULL,
    leased_until timestamp with time zone,
    lease_owner text,
    dedupe_key text,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone,
    lease_renewed_at timestamp with time zone,
    project_id uuid,
    workload_class text,
    scope text DEFAULT 'project'::text NOT NULL,
    workspace_id uuid,
    claim_seq bigint DEFAULT 0 NOT NULL,
    wakeup_generation bigint DEFAULT 1 NOT NULL,
    wakeup_emitted_generation bigint DEFAULT 0 NOT NULL,
    CONSTRAINT jobs_state_check CHECK ((state = ANY (ARRAY['queued'::text, 'capability_wait'::text, 'running'::text, 'completed'::text, 'failed'::text, 'dead'::text]))),
    CONSTRAINT jobs_workload_class_check CHECK ((workload_class = ANY (ARRAY['ai'::text, 'documents'::text, 'workflow'::text, 'integration'::text, 'projection'::text])))
);


--
-- Name: legacy_actor_mappings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.legacy_actor_mappings (
    legacy_actor_kind text NOT NULL,
    legacy_actor_id text NOT NULL,
    user_id uuid NOT NULL,
    confirmed_by uuid NOT NULL,
    confirmed_at timestamp with time zone DEFAULT now() NOT NULL,
    source text NOT NULL,
    CONSTRAINT legacy_actor_mappings_legacy_actor_id_check CHECK (((btrim(legacy_actor_id) <> ''::text) AND (length(legacy_actor_id) <= 200))),
    CONSTRAINT legacy_actor_mappings_legacy_actor_kind_check CHECK ((legacy_actor_kind = 'user'::text)),
    CONSTRAINT legacy_actor_mappings_source_check CHECK (((length(btrim(source)) >= 1) AND (length(btrim(source)) <= 200)))
);


--
-- Name: TABLE legacy_actor_mappings; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.legacy_actor_mappings IS 'Trusted operator confirmations that a historical human actor label belongs to a DeepRef user.';


--
-- Name: legacy_actor_non_identities; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.legacy_actor_non_identities (
    legacy_actor_kind text NOT NULL,
    legacy_actor_id text NOT NULL,
    user_id uuid NOT NULL,
    recorded_by uuid NOT NULL,
    recorded_at timestamp with time zone DEFAULT now() NOT NULL,
    source text NOT NULL,
    CONSTRAINT legacy_actor_non_identities_legacy_actor_id_check CHECK (((btrim(legacy_actor_id) <> ''::text) AND (length(legacy_actor_id) <= 200))),
    CONSTRAINT legacy_actor_non_identities_legacy_actor_kind_check CHECK ((legacy_actor_kind = 'user'::text)),
    CONSTRAINT legacy_actor_non_identities_source_check CHECK (((length(btrim(source)) >= 1) AND (length(btrim(source)) <= 200)))
);


--
-- Name: TABLE legacy_actor_non_identities; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.legacy_actor_non_identities IS 'Trusted operator confirmations that a historical human actor label does not belong to a DeepRef user.';


--
-- Name: metric_snapshots; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.metric_snapshots (
    project_id uuid NOT NULL,
    revision bigint NOT NULL,
    metrics_as_of timestamp with time zone NOT NULL,
    work_count bigint NOT NULL,
    edge_count bigint NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL
);


--
-- Name: notification_revision_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.notification_revision_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: notifications; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.notifications (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    revision bigint DEFAULT nextval('public.notification_revision_seq'::regclass) NOT NULL,
    kind text NOT NULL,
    severity text NOT NULL,
    project_id uuid,
    title text NOT NULL,
    body text,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    read_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT notifications_body_check CHECK (((body IS NULL) OR (length(btrim(body)) <= 500))),
    CONSTRAINT notifications_kind_check CHECK (((length(btrim(kind)) >= 1) AND (length(btrim(kind)) <= 100))),
    CONSTRAINT notifications_payload_check CHECK ((jsonb_typeof(payload) = 'object'::text)),
    CONSTRAINT notifications_severity_check CHECK ((severity = ANY (ARRAY['success'::text, 'info'::text, 'warning'::text, 'error'::text]))),
    CONSTRAINT notifications_title_check CHECK (((length(btrim(title)) >= 1) AND (length(btrim(title)) <= 200)))
);


--
-- Name: TABLE notifications; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.notifications IS 'Durable human-readable records of server-side async outcomes (imports, review runs, automations).';


--
-- Name: object_lifecycle; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.object_lifecycle (
    storage_key text NOT NULL,
    user_id uuid,
    workspace_id uuid,
    project_id uuid,
    cleanup_after timestamp with time zone,
    retired_at timestamp with time zone,
    deleted_at timestamp with time zone,
    cleanup_attempts bigint DEFAULT 0 NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    privacy_due_at timestamp with time zone
);


--
-- Name: object_retained_references; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.object_retained_references (
    reference_id uuid NOT NULL,
    storage_key text NOT NULL,
    expires_at timestamp with time zone NOT NULL
);


--
-- Name: object_storage_rollout; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.object_storage_rollout (
    singleton boolean DEFAULT true NOT NULL,
    tenant_keys_activated boolean DEFAULT false NOT NULL,
    activated_at timestamp with time zone,
    parser_rollback_floor text,
    CONSTRAINT object_storage_rollout_check CHECK ((((NOT tenant_keys_activated) AND (activated_at IS NULL) AND (parser_rollback_floor IS NULL)) OR (tenant_keys_activated AND (activated_at IS NOT NULL) AND (parser_rollback_floor IS NOT NULL) AND (parser_rollback_floor = 'tenant-object-keys-v1'::text)))),
    CONSTRAINT object_storage_rollout_singleton_check CHECK (singleton)
);


--
-- Name: operational_rate_limits; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.operational_rate_limits (
    subject_kind text NOT NULL,
    subject_id uuid NOT NULL,
    api_key_id uuid,
    workflow_id uuid,
    window_started_at timestamp with time zone NOT NULL,
    last_allowed boolean NOT NULL,
    used integer NOT NULL,
    ceiling integer NOT NULL,
    CONSTRAINT operational_rate_limits_ceiling_check CHECK (((ceiling >= 1) AND (ceiling <= 10000))),
    CONSTRAINT operational_rate_limits_check CHECK ((((subject_kind = 'api_key'::text) AND (api_key_id IS NOT NULL) AND (api_key_id = subject_id) AND (workflow_id IS NULL)) OR ((subject_kind = ANY (ARRAY['workflow_webhook'::text, 'workflow_email'::text])) AND (workflow_id IS NOT NULL) AND (workflow_id = subject_id) AND (api_key_id IS NULL)))),
    CONSTRAINT operational_rate_limits_subject_kind_check CHECK ((subject_kind = ANY (ARRAY['api_key'::text, 'workflow_webhook'::text, 'workflow_email'::text]))),
    CONSTRAINT operational_rate_limits_used_check CHECK (((used >= 1) AND (used <= 10000)))
);


--
-- Name: outbox_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.outbox_events (
    event_id uuid NOT NULL,
    message_type text NOT NULL,
    message_version integer NOT NULL,
    payload jsonb NOT NULL,
    expected_consumers text[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    delivery_state text DEFAULT 'pending'::text NOT NULL,
    published_at timestamp with time zone,
    delivered_at timestamp with time zone,
    attempts integer DEFAULT 0 NOT NULL,
    max_attempts integer DEFAULT 8 NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    lease_owner text,
    lease_expires_at timestamp with time zone,
    claim_generation bigint DEFAULT 0 NOT NULL,
    publication_owner text,
    publication_until timestamp with time zone,
    publication_generation bigint DEFAULT 0 NOT NULL,
    publication_epoch bigint DEFAULT 0 NOT NULL,
    publication_attempts integer DEFAULT 0 NOT NULL,
    publication_next_at timestamp with time zone DEFAULT now() NOT NULL,
    publication_error text,
    CONSTRAINT outbox_events_attempts_check CHECK ((attempts >= 0)),
    CONSTRAINT outbox_events_check CHECK (((lease_owner IS NULL) = (lease_expires_at IS NULL))),
    CONSTRAINT outbox_events_check1 CHECK (((delivery_state = 'delivered'::text) = (delivered_at IS NOT NULL))),
    CONSTRAINT outbox_events_claim_generation_check CHECK ((claim_generation >= 0)),
    CONSTRAINT outbox_events_delivery_state_check CHECK ((delivery_state = ANY (ARRAY['pending'::text, 'delivered'::text, 'quarantined'::text]))),
    CONSTRAINT outbox_events_expected_consumers_check CHECK ((cardinality(expected_consumers) > 0)),
    CONSTRAINT outbox_events_max_attempts_check CHECK ((max_attempts > 0)),
    CONSTRAINT outbox_events_message_version_check CHECK ((message_version > 0)),
    CONSTRAINT outbox_publication_lease_pair CHECK (((publication_owner IS NULL) = (publication_until IS NULL)))
);


--
-- Name: outbox_receipts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.outbox_receipts (
    event_id uuid NOT NULL,
    consumer text NOT NULL,
    state text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    attempt_generation bigint DEFAULT 0 NOT NULL,
    settled_generation bigint DEFAULT 0 NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    delivered_at timestamp with time zone,
    CONSTRAINT outbox_receipts_attempt_generation_check CHECK ((attempt_generation >= 0)),
    CONSTRAINT outbox_receipts_attempts_check CHECK ((attempts >= 0)),
    CONSTRAINT outbox_receipts_check CHECK (((settled_generation >= 0) AND (settled_generation <= attempt_generation))),
    CONSTRAINT outbox_receipts_check1 CHECK (((state = 'delivered'::text) = (delivered_at IS NOT NULL))),
    CONSTRAINT outbox_receipts_state_check CHECK ((state = ANY (ARRAY['retrying'::text, 'delivered'::text, 'quarantined'::text])))
);


--
-- Name: pmid_import_items; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.pmid_import_items (
    acquisition_run_id uuid NOT NULL,
    pmid text NOT NULL,
    "position" integer NOT NULL,
    status text NOT NULL,
    title text,
    record_id uuid,
    last_error text,
    queued_at timestamp with time zone DEFAULT now() NOT NULL,
    processed_at timestamp with time zone,
    CONSTRAINT pmid_import_items_pmid_check CHECK ((pmid ~ '^[1-9][0-9]{0,8}$'::text)),
    CONSTRAINT pmid_import_items_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'imported'::text, 'already_in_project'::text, 'not_found'::text, 'failed'::text])))
);


--
-- Name: prisma_snapshots; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.prisma_snapshots (
    project_id uuid NOT NULL,
    records_identified bigint DEFAULT 0 NOT NULL,
    records_deduplicated bigint DEFAULT 0 NOT NULL,
    title_abstract_pending bigint DEFAULT 0 NOT NULL,
    title_abstract_included bigint DEFAULT 0 NOT NULL,
    title_abstract_excluded bigint DEFAULT 0 NOT NULL,
    full_text_pending bigint DEFAULT 0 NOT NULL,
    full_text_included bigint DEFAULT 0 NOT NULL,
    full_text_excluded bigint DEFAULT 0 NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: project_access; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.project_access (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role text NOT NULL,
    grant_source text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT project_access_grant_source_check CHECK ((grant_source = ANY (ARRAY['workos_mirrored'::text, 'local'::text]))),
    CONSTRAINT project_access_role_check CHECK ((role = ANY (ARRAY['project_admin'::text, 'member'::text, 'viewer'::text])))
);


--
-- Name: project_ai_autonomy; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.project_ai_autonomy (
    project_id uuid NOT NULL,
    task text NOT NULL,
    level text NOT NULL,
    updated_by_kind text NOT NULL,
    updated_by_id text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT project_ai_autonomy_level_allowed CHECK ((((task = ANY (ARRAY['title_abstract_screening'::text, 'full_text_screening'::text])) AND (level <> 'act'::text)) OR ((task = ANY (ARRAY['appraisal'::text, 'study_grouping'::text])) AND (level = ANY (ARRAY['off'::text, 'suggest'::text]))) OR ((task = 'exact_duplicates'::text) AND (level = ANY (ARRAY['suggest'::text, 'act'::text]))) OR ((task = ANY (ARRAY['fuzzy_duplicates'::text, 'extraction'::text])) AND (level <> 'second_reviewer'::text)))),
    CONSTRAINT project_ai_autonomy_level_check CHECK ((level = ANY (ARRAY['off'::text, 'suggest'::text, 'second_reviewer'::text, 'act'::text]))),
    CONSTRAINT project_ai_autonomy_task_check CHECK ((task = ANY (ARRAY['exact_duplicates'::text, 'fuzzy_duplicates'::text, 'title_abstract_screening'::text, 'full_text_screening'::text, 'extraction'::text, 'appraisal'::text, 'study_grouping'::text])))
);


--
-- Name: project_ai_screening_authority; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.project_ai_screening_authority (
    project_id uuid NOT NULL,
    stage text DEFAULT 'title_abstract'::text NOT NULL,
    ceiling text NOT NULL,
    suspended_reason text,
    approved_by text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    next_audit_ordinal integer DEFAULT 1 NOT NULL,
    approved_by_actor_user_id uuid,
    CONSTRAINT project_ai_screening_authority_ceiling_check CHECK ((ceiling = ANY (ARRAY['off'::text, 'routing'::text, 'cohort_finalization'::text]))),
    CONSTRAINT project_ai_screening_authority_next_audit_ordinal_check CHECK (((next_audit_ordinal >= 1) AND (next_audit_ordinal <= 26))),
    CONSTRAINT project_ai_screening_authority_stage_check CHECK ((stage = 'title_abstract'::text))
);


--
-- Name: project_reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.project_reports (
    project_id uuid NOT NULL,
    report_id uuid NOT NULL,
    first_seen_record_id uuid,
    lifecycle_status text DEFAULT 'discovered'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    total_citations bigint DEFAULT 0 NOT NULL,
    references_count bigint DEFAULT 0 NOT NULL,
    internal_citations bigint DEFAULT 0 NOT NULL,
    outbound_internal_references bigint DEFAULT 0 NOT NULL,
    rank_score double precision DEFAULT 0 NOT NULL,
    metrics_computed_at timestamp with time zone,
    CONSTRAINT project_reports_lifecycle_status_check CHECK ((lifecycle_status = ANY (ARRAY['discovered'::text, 'screening'::text, 'included'::text, 'excluded'::text, 'maybe'::text])))
);


--
-- Name: project_unresolved_references; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.project_unresolved_references (
    id text NOT NULL,
    project_id uuid NOT NULL,
    source_report_id uuid NOT NULL,
    raw_unstructured text,
    article_title text,
    author text,
    year text,
    volume text,
    first_page text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: projection_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.projection_state (
    projection_name text NOT NULL,
    project_id uuid,
    state text DEFAULT 'pending'::text NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    watermark bigint DEFAULT 0 NOT NULL,
    lag bigint DEFAULT 0 NOT NULL,
    last_success_at timestamp with time zone,
    last_error text,
    rebuild_state text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    generation bigint DEFAULT 0 NOT NULL,
    attempt bigint DEFAULT 0 NOT NULL,
    CONSTRAINT projection_state_attempt_check CHECK ((attempt >= 0)),
    CONSTRAINT projection_state_generation_check CHECK ((generation >= 0)),
    CONSTRAINT projection_state_lag_check CHECK ((lag >= 0))
);


--
-- Name: projects; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.projects (
    id uuid NOT NULL,
    name text NOT NULL,
    description text,
    default_max_depth integer DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    ai_monthly_budget_micros bigint DEFAULT 5000000 NOT NULL,
    historical_default_max_depth integer,
    metric_requested_revision bigint DEFAULT 0 NOT NULL,
    workspace_id uuid,
    metric_projection_generation bigint DEFAULT 0 NOT NULL,
    metric_projection_attempt bigint DEFAULT 0 NOT NULL,
    CONSTRAINT projects_ai_monthly_budget_micros_check CHECK ((ai_monthly_budget_micros >= 0)),
    CONSTRAINT projects_depth_bounded CHECK ((((default_max_depth >= 0) AND (default_max_depth <= 3)) OR ((historical_default_max_depth IS NOT NULL) AND (default_max_depth = historical_default_max_depth)))),
    CONSTRAINT projects_metric_projection_attempt_check CHECK ((metric_projection_attempt >= 0)),
    CONSTRAINT projects_metric_projection_generation_check CHECK ((metric_projection_generation >= 0)),
    CONSTRAINT projects_metric_requested_revision_check CHECK ((metric_requested_revision >= 0))
);


--
-- Name: protocol_versions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.protocol_versions (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    version integer NOT NULL,
    name text NOT NULL,
    status text NOT NULL,
    criteria jsonb DEFAULT '[]'::jsonb NOT NULL,
    published_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    framework_kind text DEFAULT 'custom'::text NOT NULL,
    framework_fields jsonb DEFAULT '{}'::jsonb NOT NULL,
    objective text DEFAULT ''::text NOT NULL,
    question text DEFAULT ''::text NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    amendment_of uuid,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    created_by_kind text DEFAULT 'system'::text NOT NULL,
    created_by_id text DEFAULT 'migration'::text NOT NULL,
    updated_by_kind text DEFAULT 'system'::text NOT NULL,
    updated_by_id text DEFAULT 'migration'::text NOT NULL,
    published_by_kind text,
    published_by_id text,
    CONSTRAINT protocol_versions_amendment_not_self_check CHECK (((amendment_of IS NULL) OR (amendment_of <> id))),
    CONSTRAINT protocol_versions_framework_kind_check CHECK ((framework_kind = ANY (ARRAY['pico'::text, 'picos'::text, 'peco'::text, 'peo'::text, 'pcc'::text, 'spider'::text, 'custom'::text]))),
    CONSTRAINT protocol_versions_revision_check CHECK ((revision >= 1)),
    CONSTRAINT protocol_versions_status_check CHECK ((status = ANY (ARRAY['draft'::text, 'published'::text, 'superseded'::text]))),
    CONSTRAINT protocol_versions_version_positive_check CHECK ((version >= 1))
);


--
-- Name: COLUMN protocol_versions.criteria; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.protocol_versions.criteria IS 'Legacy compatibility projection; new protocol writes use eligibility_criteria.';


--
-- Name: COLUMN protocol_versions.framework_fields; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.protocol_versions.framework_fields IS 'Structured framework fields validated by the domain boundary.';


--
-- Name: provider_rate_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.provider_rate_state (
    provider text NOT NULL,
    next_permit_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: record_identifiers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.record_identifiers (
    id uuid NOT NULL,
    record_id uuid NOT NULL,
    scheme text NOT NULL,
    value text NOT NULL,
    normalized_value text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: records; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.records (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    report_id uuid,
    source text NOT NULL,
    source_key text,
    title text,
    abstract_text text,
    publication_year integer,
    raw jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    acquisition_run_id uuid,
    journal text,
    authors jsonb DEFAULT '[]'::jsonb NOT NULL,
    source_identifiers jsonb DEFAULT '[]'::jsonb NOT NULL,
    normalized_title text
);


--
-- Name: report_fetch_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.report_fetch_state (
    report_id uuid NOT NULL,
    canonical_doi text NOT NULL,
    provider text DEFAULT 'crossref'::text NOT NULL,
    status text DEFAULT 'queued'::text NOT NULL,
    locked_at timestamp with time zone,
    fetched_at timestamp with time zone,
    last_error text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    owner_token uuid,
    lease_expires_at timestamp with time zone,
    heartbeat_at timestamp with time zone,
    attempts integer DEFAULT 0 NOT NULL,
    CONSTRAINT report_fetch_state_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'fetching'::text, 'fetched'::text, 'not_found'::text, 'failed'::text])))
);


--
-- Name: report_identifiers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.report_identifiers (
    id uuid NOT NULL,
    report_id uuid NOT NULL,
    scheme text NOT NULL,
    value text NOT NULL,
    normalized_value text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.reports (
    id uuid NOT NULL,
    title text,
    abstract_text text,
    publication_year integer,
    journal text,
    url text,
    raw jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    work_type text,
    publisher text,
    container_title text,
    total_citations bigint DEFAULT 0 NOT NULL,
    references_count bigint DEFAULT 0 NOT NULL,
    normalized_title text,
    authors jsonb DEFAULT '[]'::jsonb NOT NULL
);


--
-- Name: review_artifact_lineage; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.review_artifact_lineage (
    project_id uuid NOT NULL,
    artifact_id uuid NOT NULL,
    predecessor_artifact_id uuid NOT NULL,
    CONSTRAINT review_artifact_lineage_check CHECK ((artifact_id <> predecessor_artifact_id))
);


--
-- Name: review_artifacts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.review_artifacts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    content_hash text NOT NULL,
    media_type text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT review_artifacts_content_hash_check CHECK ((content_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT review_artifacts_media_type_check CHECK (((length(btrim(media_type)) >= 1) AND (length(btrim(media_type)) <= 200)))
);


--
-- Name: review_attempt_details; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.review_attempt_details (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    run_id uuid NOT NULL,
    node_id text NOT NULL,
    node_version integer NOT NULL,
    input_fingerprint text NOT NULL,
    artifact_id uuid,
    model_run_id uuid,
    error_code text,
    error_message text,
    accepted_at timestamp with time zone,
    accepted_attempt_id uuid,
    CONSTRAINT review_attempt_details_node_version_check CHECK ((node_version > 0))
);


--
-- Name: review_calibration_bundles; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.review_calibration_bundles (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    definition_key text NOT NULL,
    semantic_bundle_hash text NOT NULL,
    evaluation_set_id text NOT NULL,
    thresholds jsonb NOT NULL,
    metrics jsonb NOT NULL,
    reviewer_metadata jsonb NOT NULL,
    status text NOT NULL,
    evaluated_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    stage text,
    identity_scheme integer NOT NULL,
    identity_snapshot jsonb,
    CONSTRAINT review_calibration_bundles_definition_key_check CHECK ((definition_key = ANY (ARRAY['screening'::text, 'duplicate_detection'::text, 'study_classification'::text, 'study_grouping'::text, 'appraisal_prefill'::text, 'data_extraction'::text]))),
    CONSTRAINT review_calibration_bundles_evaluation_set_id_check CHECK (((length(btrim(evaluation_set_id)) >= 1) AND (length(btrim(evaluation_set_id)) <= 500))),
    CONSTRAINT review_calibration_bundles_identity_shape_check CHECK (((((identity_scheme = 1) AND (identity_snapshot IS NULL) AND (stage IS NULL)) OR ((identity_scheme = 2) AND (identity_snapshot IS NOT NULL) AND (jsonb_typeof(identity_snapshot) = 'object'::text) AND ((identity_snapshot -> 'scheme'::text) = to_jsonb(identity_scheme)) AND ((identity_snapshot ->> 'definition'::text) = definition_key) AND (NOT ((identity_snapshot ->> 'stage'::text) IS DISTINCT FROM stage)) AND (jsonb_typeof((identity_snapshot -> 'components'::text)) = 'object'::text)) OR ((identity_scheme = 3) AND (identity_snapshot IS NOT NULL) AND (jsonb_typeof(identity_snapshot) = 'object'::text) AND ((identity_snapshot -> 'scheme'::text) = to_jsonb(identity_scheme)) AND ((identity_snapshot ->> 'definition'::text) = definition_key) AND (NOT ((identity_snapshot ->> 'stage'::text) IS DISTINCT FROM stage)) AND (jsonb_typeof((identity_snapshot -> 'semantic_version'::text)) = 'number'::text) AND (jsonb_typeof((identity_snapshot -> 'models'::text)) = 'array'::text))) IS TRUE)),
    CONSTRAINT review_calibration_bundles_metrics_check CHECK ((jsonb_typeof(metrics) = 'object'::text)),
    CONSTRAINT review_calibration_bundles_reviewer_metadata_check CHECK ((jsonb_typeof(reviewer_metadata) = 'object'::text)),
    CONSTRAINT review_calibration_bundles_semantic_bundle_hash_check CHECK ((semantic_bundle_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT review_calibration_bundles_stage_check CHECK ((stage = ANY (ARRAY['title_abstract'::text, 'full_text'::text]))),
    CONSTRAINT review_calibration_bundles_stage_scope_check CHECK ((((definition_key = 'screening'::text) AND ((stage IS NOT NULL) OR (identity_scheme = 1))) OR ((definition_key <> 'screening'::text) AND (stage IS NULL)))),
    CONSTRAINT review_calibration_bundles_status_check CHECK ((status = ANY (ARRAY['passing'::text, 'failed'::text]))),
    CONSTRAINT review_calibration_bundles_thresholds_check CHECK ((jsonb_typeof(thresholds) = 'object'::text))
);


--
-- Name: review_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.review_events (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    event_type text NOT NULL,
    aggregate_type text NOT NULL,
    aggregate_id uuid NOT NULL,
    payload jsonb NOT NULL,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: review_proposal_finalizations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.review_proposal_finalizations (
    project_id uuid NOT NULL,
    automation_run_id uuid NOT NULL,
    candidate_hash text NOT NULL,
    proposal_id uuid NOT NULL,
    model_run_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT review_proposal_finalizations_candidate_hash_check CHECK ((candidate_hash ~ '^[0-9a-f]{64}$'::text))
);


--
-- Name: review_run_manifests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.review_run_manifests (
    project_id uuid NOT NULL,
    automation_run_id uuid NOT NULL,
    definition_key text NOT NULL,
    definition_id text NOT NULL,
    definition_version integer NOT NULL,
    manifest_hash text NOT NULL,
    semantic_bundle_hash text NOT NULL,
    manifest jsonb NOT NULL,
    subject jsonb NOT NULL,
    origin jsonb NOT NULL,
    prepared_task jsonb NOT NULL,
    state_code text,
    state_message text,
    candidate_hash text,
    proposal_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    generation integer DEFAULT 0 NOT NULL,
    reuse_epoch integer DEFAULT 0 NOT NULL,
    CONSTRAINT review_run_manifests_candidate_hash_check CHECK (((candidate_hash IS NULL) OR (candidate_hash ~ '^[0-9a-f]{64}$'::text))),
    CONSTRAINT review_run_manifests_definition_id_check CHECK (((length(btrim(definition_id)) >= 1) AND (length(btrim(definition_id)) <= 200))),
    CONSTRAINT review_run_manifests_definition_key_check CHECK ((definition_key = ANY (ARRAY['screening'::text, 'duplicate_detection'::text, 'study_classification'::text, 'study_grouping'::text, 'appraisal_prefill'::text, 'data_extraction'::text]))),
    CONSTRAINT review_run_manifests_definition_version_check CHECK ((definition_version > 0)),
    CONSTRAINT review_run_manifests_generation_check CHECK (((generation >= 0) AND (reuse_epoch >= 0) AND (reuse_epoch <= generation))),
    CONSTRAINT review_run_manifests_manifest_check CHECK ((jsonb_typeof(manifest) = 'object'::text)),
    CONSTRAINT review_run_manifests_manifest_hash_check CHECK ((manifest_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT review_run_manifests_origin_check CHECK ((jsonb_typeof(origin) = 'object'::text)),
    CONSTRAINT review_run_manifests_prepared_task_check CHECK ((jsonb_typeof(prepared_task) = 'object'::text)),
    CONSTRAINT review_run_manifests_semantic_bundle_hash_check CHECK ((semantic_bundle_hash ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT review_run_manifests_state_code_check CHECK (((state_code IS NULL) OR ((length(btrim(state_code)) >= 1) AND (length(btrim(state_code)) <= 100)))),
    CONSTRAINT review_run_manifests_state_message_check CHECK (((state_message IS NULL) OR ((length(btrim(state_message)) >= 1) AND (length(btrim(state_message)) <= 4096)))),
    CONSTRAINT review_run_manifests_subject_check CHECK ((jsonb_typeof(subject) = 'object'::text))
);


--
-- Name: scientific_assignments; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.scientific_assignments (
    project_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role text NOT NULL,
    assigned_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone,
    CONSTRAINT scientific_assignments_check CHECK (((revoked_at IS NULL) OR (revoked_at >= created_at))),
    CONSTRAINT scientific_assignments_role_check CHECK ((role = ANY (ARRAY['primary_screener'::text, 'secondary_screener'::text, 'consensus_reviewer'::text, 'extractor'::text, 'appraisal_reviewer'::text, 'fresh_ai_auditor'::text])))
);


--
-- Name: TABLE scientific_assignments; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.scientific_assignments IS 'Methodology roles, independent of workspace or WorkOS account roles. Revocation preserves assignment history.';


--
-- Name: screening_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.screening_events (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    report_id uuid NOT NULL,
    stage text NOT NULL,
    decision text,
    exclusion_reason_id uuid,
    notes text,
    protocol_version_id uuid NOT NULL,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    supersedes_event_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    event_kind text DEFAULT 'decision'::text NOT NULL,
    undoes_event_id uuid,
    previous_title_abstract_status text DEFAULT 'unscreened'::text NOT NULL,
    previous_full_text_status text DEFAULT 'not_required'::text NOT NULL,
    previous_full_text_exclusion_reason_id uuid,
    previous_final_status text DEFAULT 'unscreened'::text NOT NULL,
    result_title_abstract_status text DEFAULT 'unscreened'::text NOT NULL,
    result_full_text_status text DEFAULT 'not_required'::text NOT NULL,
    result_full_text_exclusion_reason_id uuid,
    result_final_status text DEFAULT 'unscreened'::text NOT NULL,
    full_text_reason_stage text GENERATED ALWAYS AS ('full_text'::text) STORED,
    origin text,
    actor_user_id uuid,
    CONSTRAINT screening_events_actor_id_check CHECK ((length(btrim(actor_id)) > 0)),
    CONSTRAINT screening_events_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT screening_events_decision_check CHECK ((decision = ANY (ARRAY['include'::text, 'exclude'::text, 'maybe'::text]))),
    CONSTRAINT screening_events_event_kind_check CHECK ((event_kind = ANY (ARRAY['decision'::text, 'undo'::text]))),
    CONSTRAINT screening_events_event_shape_check CHECK ((((event_kind = 'decision'::text) AND (decision IS NOT NULL) AND (undoes_event_id IS NULL) AND (((stage = 'title_abstract'::text) AND (exclusion_reason_id IS NULL)) OR ((stage = 'full_text'::text) AND (((decision = 'exclude'::text) AND (exclusion_reason_id IS NOT NULL)) OR ((decision = ANY (ARRAY['include'::text, 'maybe'::text])) AND (exclusion_reason_id IS NULL)))))) OR ((event_kind = 'undo'::text) AND (decision IS NULL) AND (exclusion_reason_id IS NULL) AND (undoes_event_id IS NOT NULL)))),
    CONSTRAINT screening_events_origin_check CHECK (((origin IS NULL) OR (origin = ANY (ARRAY['direct'::text, 'assistant_plan'::text])))),
    CONSTRAINT screening_events_previous_final_status_check CHECK ((previous_final_status = ANY (ARRAY['unscreened'::text, 'pending_full_text'::text, 'include'::text, 'exclude'::text, 'maybe'::text]))),
    CONSTRAINT screening_events_previous_full_status_check CHECK ((previous_full_text_status = ANY (ARRAY['not_required'::text, 'unscreened'::text, 'include'::text, 'exclude'::text, 'maybe'::text]))),
    CONSTRAINT screening_events_previous_reason_shape_check CHECK ((((previous_full_text_status = 'exclude'::text) AND (previous_full_text_exclusion_reason_id IS NOT NULL)) OR ((previous_full_text_status <> 'exclude'::text) AND (previous_full_text_exclusion_reason_id IS NULL)))),
    CONSTRAINT screening_events_previous_title_status_check CHECK ((previous_title_abstract_status = ANY (ARRAY['unscreened'::text, 'include'::text, 'exclude'::text, 'maybe'::text]))),
    CONSTRAINT screening_events_result_final_status_check CHECK ((result_final_status = ANY (ARRAY['unscreened'::text, 'pending_full_text'::text, 'include'::text, 'exclude'::text, 'maybe'::text]))),
    CONSTRAINT screening_events_result_full_status_check CHECK ((result_full_text_status = ANY (ARRAY['not_required'::text, 'unscreened'::text, 'include'::text, 'exclude'::text, 'maybe'::text]))),
    CONSTRAINT screening_events_result_reason_shape_check CHECK ((((result_full_text_status = 'exclude'::text) AND (result_full_text_exclusion_reason_id IS NOT NULL)) OR ((result_full_text_status <> 'exclude'::text) AND (result_full_text_exclusion_reason_id IS NULL)))),
    CONSTRAINT screening_events_result_title_status_check CHECK ((result_title_abstract_status = ANY (ARRAY['unscreened'::text, 'include'::text, 'exclude'::text, 'maybe'::text]))),
    CONSTRAINT screening_events_stage_check CHECK ((stage = ANY (ARRAY['title_abstract'::text, 'full_text'::text])))
);


--
-- Name: screening_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.screening_state (
    project_id uuid NOT NULL,
    report_id uuid NOT NULL,
    title_abstract_status text DEFAULT 'unscreened'::text NOT NULL,
    full_text_status text DEFAULT 'not_required'::text NOT NULL,
    full_text_exclusion_reason_id uuid,
    final_status text DEFAULT 'unscreened'::text NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    last_event_id uuid,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    full_text_reason_stage text GENERATED ALWAYS AS ('full_text'::text) STORED,
    CONSTRAINT screening_state_final_status_check CHECK ((final_status = ANY (ARRAY['unscreened'::text, 'pending_full_text'::text, 'include'::text, 'exclude'::text, 'maybe'::text]))),
    CONSTRAINT screening_state_full_text_status_check CHECK ((full_text_status = ANY (ARRAY['not_required'::text, 'unscreened'::text, 'include'::text, 'exclude'::text, 'maybe'::text]))),
    CONSTRAINT screening_state_reason_shape_check CHECK ((((full_text_status = 'exclude'::text) AND (full_text_exclusion_reason_id IS NOT NULL)) OR ((full_text_status <> 'exclude'::text) AND (full_text_exclusion_reason_id IS NULL)))),
    CONSTRAINT screening_state_revision_check CHECK ((revision >= 0)),
    CONSTRAINT screening_state_stage_shape_check CHECK (((title_abstract_status = 'include'::text) OR ((full_text_status = 'not_required'::text) AND (full_text_exclusion_reason_id IS NULL)))),
    CONSTRAINT screening_state_title_abstract_status_check CHECK ((title_abstract_status = ANY (ARRAY['unscreened'::text, 'include'::text, 'exclude'::text, 'maybe'::text])))
);


--
-- Name: second_review_attempts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.second_review_attempts (
    project_id uuid NOT NULL,
    report_id uuid NOT NULL,
    stage text NOT NULL,
    protocol_version_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    review_run_id uuid,
    attempts integer DEFAULT 1 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT second_review_attempts_attempts_check CHECK ((attempts >= 1)),
    CONSTRAINT second_review_attempts_stage_check CHECK ((stage = ANY (ARRAY['title_abstract'::text, 'full_text'::text])))
);


--
-- Name: second_review_gate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.second_review_gate (
    project_id uuid NOT NULL,
    stage text NOT NULL,
    refusal text NOT NULL,
    observed_at timestamp with time zone DEFAULT now() NOT NULL,
    reasons text[] DEFAULT '{}'::text[] NOT NULL,
    CONSTRAINT second_review_gate_refusal_check CHECK ((refusal = ANY (ARRAY['calibration_stale'::text, 'calibration_stage_mismatch'::text, 'calibration_incompatible'::text]))),
    CONSTRAINT second_review_gate_stage_check CHECK ((stage = ANY (ARRAY['title_abstract'::text, 'full_text'::text])))
);


--
-- Name: settings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.settings (
    id integer DEFAULT 1 NOT NULL,
    default_max_depth integer DEFAULT 1 NOT NULL,
    max_concurrency integer DEFAULT 8 NOT NULL,
    rate_limit_per_second integer DEFAULT 1 NOT NULL,
    retry_attempts integer DEFAULT 5 NOT NULL,
    metadata_provider text DEFAULT 'crossref'::text NOT NULL,
    citation_provider text DEFAULT 'crossref'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    historical_default_max_depth integer,
    CONSTRAINT settings_depth_bounded CHECK ((((default_max_depth >= 0) AND (default_max_depth <= 3)) OR ((historical_default_max_depth IS NOT NULL) AND (default_max_depth = historical_default_max_depth)))),
    CONSTRAINT settings_id_check CHECK ((id = 1))
);


--
-- Name: studies; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.studies (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    title text NOT NULL,
    design text,
    raw jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    study_revision bigint DEFAULT 0 NOT NULL,
    design_context jsonb DEFAULT '{}'::jsonb NOT NULL,
    updated_by_actor_kind text DEFAULT 'system'::text NOT NULL,
    updated_by_actor_id text DEFAULT 'legacy-migration'::text NOT NULL,
    classified_at timestamp with time zone,
    CONSTRAINT studies_design_check CHECK (((design IS NULL) OR (design = ANY (ARRAY['rct'::text, 'non_randomized_intervention'::text, 'cohort'::text, 'case_control'::text, 'cross_sectional'::text, 'diagnostic_accuracy'::text, 'prediction_model'::text, 'qualitative'::text, 'systematic_review'::text, 'case_series'::text])))),
    CONSTRAINT studies_design_context_object_check CHECK ((jsonb_typeof(design_context) = 'object'::text)),
    CONSTRAINT studies_revision_check CHECK ((study_revision >= 0)),
    CONSTRAINT studies_title_check CHECK (((length(btrim(title)) > 0) AND (char_length(title) <= 200))),
    CONSTRAINT studies_updated_actor_id_check CHECK ((length(btrim(updated_by_actor_id)) > 0)),
    CONSTRAINT studies_updated_actor_kind_check CHECK ((updated_by_actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text])))
);


--
-- Name: study_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.study_events (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    study_id uuid NOT NULL,
    report_id uuid,
    event_type text NOT NULL,
    before_study_id uuid,
    result_study_id uuid,
    before_revision bigint NOT NULL,
    result_revision bigint NOT NULL,
    before_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    result_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    actor_kind text NOT NULL,
    actor_id text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT study_events_actor_id_check CHECK ((length(btrim(actor_id)) > 0)),
    CONSTRAINT study_events_actor_kind_check CHECK ((actor_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT study_events_before_revision_check CHECK ((before_revision >= 0)),
    CONSTRAINT study_events_before_snapshot_check CHECK ((jsonb_typeof(before_snapshot) = 'object'::text)),
    CONSTRAINT study_events_event_type_check CHECK ((event_type = ANY (ARRAY['study_created'::text, 'study_renamed'::text, 'report_assigned'::text, 'report_moved'::text, 'report_unassigned'::text, 'study_classified'::text]))),
    CONSTRAINT study_events_payload_check CHECK ((jsonb_typeof(payload) = 'object'::text)),
    CONSTRAINT study_events_result_revision_check CHECK ((result_revision >= 0)),
    CONSTRAINT study_events_result_snapshot_check CHECK ((jsonb_typeof(result_snapshot) = 'object'::text))
);


--
-- Name: study_reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.study_reports (
    study_id uuid NOT NULL,
    report_id uuid NOT NULL,
    relationship text DEFAULT 'report_of_study'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    project_id uuid NOT NULL,
    CONSTRAINT study_reports_relationship_check CHECK ((relationship = ANY (ARRAY['report_of_study'::text, 'protocol'::text, 'primary_outcome'::text, 'safety_analysis'::text, 'economic_analysis'::text, 'follow_up'::text])))
);


--
-- Name: transport_dead_letters; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.transport_dead_letters (
    delivery_id text NOT NULL,
    event_id uuid,
    consumer text NOT NULL,
    publication_epoch bigint,
    reason text NOT NULL,
    deliveries bigint NOT NULL,
    recorded_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: user_sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_sessions (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    session_token_hash bytea NOT NULL,
    sealed_provider_session text NOT NULL,
    session_ref bytea NOT NULL,
    version bigint DEFAULT 0 NOT NULL,
    refresh_lease_owner uuid,
    refresh_lease_until timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    CONSTRAINT user_sessions_check CHECK (((refresh_lease_owner IS NULL) = (refresh_lease_until IS NULL))),
    CONSTRAINT user_sessions_check1 CHECK ((expires_at > created_at)),
    CONSTRAINT user_sessions_sealed_provider_session_check CHECK (((length(sealed_provider_session) >= 1) AND (length(sealed_provider_session) <= 16384))),
    CONSTRAINT user_sessions_session_ref_check CHECK ((length(session_ref) = 32)),
    CONSTRAINT user_sessions_session_token_hash_check CHECK ((length(session_token_hash) = 32))
);


--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users (
    id uuid NOT NULL,
    workos_user_id text NOT NULL,
    primary_email text NOT NULL,
    display_name text,
    status text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    verified_email text,
    onboarded_at timestamp with time zone,
    CONSTRAINT users_display_name_check CHECK (((display_name IS NULL) OR (length(display_name) <= 200))),
    CONSTRAINT users_primary_email_check CHECK (((length(primary_email) >= 1) AND (length(primary_email) <= 320))),
    CONSTRAINT users_status_check CHECK ((status = ANY (ARRAY['active'::text, 'deactivated'::text]))),
    CONSTRAINT users_workos_user_id_check CHECK (((length(workos_user_id) >= 1) AND (length(workos_user_id) <= 128)))
);


--
-- Name: worker_heartbeats; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.worker_heartbeats (
    owner text NOT NULL,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    seen_at timestamp with time zone DEFAULT now() NOT NULL,
    pool_capacities jsonb DEFAULT '{}'::jsonb NOT NULL,
    known_job_kinds text[],
    transport_mode text,
    transport_ready boolean,
    transport_observed_at timestamp with time zone,
    CONSTRAINT worker_heartbeats_owner_check CHECK (((length(owner) >= 1) AND (length(owner) <= 200))),
    CONSTRAINT worker_heartbeats_transport_mode_check CHECK ((transport_mode = ANY (ARRAY['postgres'::text, 'jetstream'::text])))
);


--
-- Name: COLUMN worker_heartbeats.known_job_kinds; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.worker_heartbeats.known_job_kinds IS 'Job kinds this worker binary decodes, written with every heartbeat. NULL means the binary predates capability declaration.';


--
-- Name: workflow_file_storage_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workflow_file_storage_state (
    singleton boolean DEFAULT true NOT NULL,
    mode text DEFAULT 'overlap'::text NOT NULL,
    changed_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT workflow_file_storage_state_mode_check CHECK ((mode = ANY (ARRAY['overlap'::text, 'object_only'::text, 'contracted'::text]))),
    CONSTRAINT workflow_file_storage_state_singleton_check CHECK (singleton)
);


--
-- Name: workflow_files; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workflow_files (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    run_id uuid NOT NULL,
    node_id text NOT NULL,
    name text NOT NULL,
    content_type text NOT NULL,
    content bytea NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    storage_key text,
    content_sha256 text,
    byte_size bigint,
    CONSTRAINT workflow_files_content_check CHECK ((octet_length(content) <= 16777216)),
    CONSTRAINT workflow_files_name_check CHECK (((length(btrim(name)) >= 1) AND (length(btrim(name)) <= 200))),
    CONSTRAINT workflow_files_object_reference_check CHECK ((((storage_key IS NULL) AND (content_sha256 IS NULL) AND (byte_size IS NULL)) OR ((storage_key IS NOT NULL) AND (content_sha256 ~ '^[0-9a-f]{64}$'::text) AND (byte_size >= 0))))
);


--
-- Name: workflow_run_details; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workflow_run_details (
    id uuid NOT NULL,
    project_id uuid NOT NULL,
    workflow_id uuid NOT NULL,
    version_id uuid,
    graph jsonb NOT NULL,
    trigger_kind text NOT NULL,
    trigger_data jsonb NOT NULL,
    idempotency_key text NOT NULL,
    test_mode boolean NOT NULL
);


--
-- Name: workflow_secrets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workflow_secrets (
    workflow_id uuid NOT NULL,
    node_id text NOT NULL,
    key text NOT NULL,
    value text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    origin text,
    CONSTRAINT workflow_secrets_value_check CHECK ((length(value) <= 4096))
);


--
-- Name: workflow_seen_items; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workflow_seen_items (
    workflow_id uuid NOT NULL,
    identifier text NOT NULL,
    first_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT workflow_seen_items_identifier_check CHECK (((length(identifier) >= 1) AND (length(identifier) <= 500)))
);


--
-- Name: workflow_step_details; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workflow_step_details (
    id uuid NOT NULL,
    run_id uuid NOT NULL,
    node_id text NOT NULL,
    iteration text DEFAULT ''::text NOT NULL,
    node_type text NOT NULL,
    input jsonb,
    fired_ports text[] DEFAULT '{}'::text[] NOT NULL,
    items integer DEFAULT 0 NOT NULL,
    note text,
    wait_state jsonb,
    CONSTRAINT workflow_step_details_items_check CHECK ((items >= 0))
);


--
-- Name: workflow_versions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workflow_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    workflow_id uuid NOT NULL,
    version integer NOT NULL,
    graph jsonb NOT NULL,
    trigger_kind text NOT NULL,
    note text,
    published_by_kind text NOT NULL,
    published_by_id text NOT NULL,
    published_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT workflow_versions_graph_check CHECK ((jsonb_typeof(graph) = 'object'::text)),
    CONSTRAINT workflow_versions_note_check CHECK (((note IS NULL) OR (length(note) <= 500))),
    CONSTRAINT workflow_versions_published_by_id_check CHECK (((length(btrim(published_by_id)) >= 1) AND (length(btrim(published_by_id)) <= 200))),
    CONSTRAINT workflow_versions_published_by_kind_check CHECK ((published_by_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT workflow_versions_version_check CHECK ((version >= 1))
);


--
-- Name: workflows; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workflows (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    project_id uuid NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    status text DEFAULT 'disabled'::text NOT NULL,
    draft_graph jsonb DEFAULT '{"edges": [], "nodes": []}'::jsonb NOT NULL,
    draft_revision bigint DEFAULT 1 NOT NULL,
    published_version_id uuid,
    trigger_kind text,
    webhook_token text NOT NULL,
    webhook_secret text NOT NULL,
    webhook_signature_required boolean DEFAULT true NOT NULL,
    email_token text NOT NULL,
    next_fire_at timestamp with time zone,
    last_fired_at timestamp with time zone,
    poll_state jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_by_kind text NOT NULL,
    created_by_id text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT workflows_created_by_id_check CHECK (((length(btrim(created_by_id)) >= 1) AND (length(btrim(created_by_id)) <= 200))),
    CONSTRAINT workflows_created_by_kind_check CHECK ((created_by_kind = ANY (ARRAY['user'::text, 'automation'::text, 'system'::text]))),
    CONSTRAINT workflows_description_check CHECK ((length(description) <= 2000)),
    CONSTRAINT workflows_draft_graph_check CHECK ((jsonb_typeof(draft_graph) = 'object'::text)),
    CONSTRAINT workflows_draft_revision_check CHECK ((draft_revision >= 1)),
    CONSTRAINT workflows_name_check CHECK (((length(btrim(name)) >= 1) AND (length(btrim(name)) <= 200))),
    CONSTRAINT workflows_poll_state_check CHECK ((jsonb_typeof(poll_state) = 'object'::text)),
    CONSTRAINT workflows_status_check CHECK ((status = ANY (ARRAY['enabled'::text, 'disabled'::text])))
);


--
-- Name: works; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.works (
    canonical_doi text NOT NULL,
    title text,
    abstract_text text,
    work_type text,
    publisher text,
    container_title text,
    issued_year integer,
    published_year integer,
    url text,
    total_citations integer DEFAULT 0 NOT NULL,
    references_count integer DEFAULT 0 NOT NULL,
    metadata_provider text DEFAULT 'crossref'::text NOT NULL,
    citation_provider text DEFAULT 'crossref'::text NOT NULL,
    fetched_at timestamp with time zone,
    raw jsonb DEFAULT '{}'::jsonb NOT NULL
);


--
-- Name: workspace_invitations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workspace_invitations (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    email text NOT NULL,
    role text NOT NULL,
    invited_by uuid NOT NULL,
    status text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    accepted_by_user_id uuid,
    accepted_at timestamp with time zone,
    revoked_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT workspace_invitations_check CHECK (((status = 'accepted'::text) = ((accepted_by_user_id IS NOT NULL) AND (accepted_at IS NOT NULL)))),
    CONSTRAINT workspace_invitations_check1 CHECK (((status = 'revoked'::text) = (revoked_at IS NOT NULL))),
    CONSTRAINT workspace_invitations_email_check CHECK (((email = lower(btrim(email))) AND ((length(email) >= 3) AND (length(email) <= 320)) AND (POSITION(('@'::text) IN (email)) > 1))),
    CONSTRAINT workspace_invitations_role_check CHECK ((role = ANY (ARRAY['owner'::text, 'admin'::text, 'member'::text]))),
    CONSTRAINT workspace_invitations_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'accepted'::text, 'revoked'::text, 'expired'::text])))
);


--
-- Name: workspace_memberships; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workspace_memberships (
    id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    workos_membership_id text,
    role text NOT NULL,
    status text NOT NULL,
    grant_source text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT workspace_memberships_check CHECK (((grant_source = 'workos_mirrored'::text) = (workos_membership_id IS NOT NULL))),
    CONSTRAINT workspace_memberships_grant_source_check CHECK ((grant_source = ANY (ARRAY['workos_mirrored'::text, 'local'::text]))),
    CONSTRAINT workspace_memberships_role_check CHECK ((role = ANY (ARRAY['owner'::text, 'admin'::text, 'member'::text]))),
    CONSTRAINT workspace_memberships_status_check CHECK ((status = ANY (ARRAY['active'::text, 'deactivated'::text]))),
    CONSTRAINT workspace_memberships_workos_membership_id_check CHECK (((workos_membership_id IS NULL) OR ((length(workos_membership_id) >= 1) AND (length(workos_membership_id) <= 128))))
);


--
-- Name: workspaces; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workspaces (
    id uuid NOT NULL,
    kind text NOT NULL,
    name text NOT NULL,
    slug text NOT NULL,
    workos_organization_id text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    workos_reconciled_through timestamp with time zone,
    CONSTRAINT workspaces_kind_check CHECK ((kind = ANY (ARRAY['personal'::text, 'organization'::text, 'migration'::text]))),
    CONSTRAINT workspaces_name_check CHECK (((length(name) >= 1) AND (length(name) <= 200))),
    CONSTRAINT workspaces_slug_check CHECK (((slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$'::text) AND (length(slug) <= 80))),
    CONSTRAINT workspaces_workos_organization_id_check CHECK (((workos_organization_id IS NULL) OR ((length(workos_organization_id) >= 1) AND (length(workos_organization_id) <= 128))))
);


--
-- Data for Name: account_data_exports; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: account_deletion_requests; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: account_erasure_objects; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: account_erasure_progress; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: account_erasure_rollout; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.account_erasure_rollout (singleton, activated, activated_at, attestation) VALUES (true, false, NULL, NULL);


--
-- Data for Name: account_erasure_writers; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: account_export_rollout; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.account_export_rollout (singleton, retention_activated, activated_at, operator_attestation) VALUES (true, false, NULL, NULL);


--
-- Data for Name: account_export_writers; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: account_human_activity; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: acquisition_items; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: acquisition_runs; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: admin_audit_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_activity; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_autonomy_outbox; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_concurrency_slots; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_model_routes; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_opinion_exposures; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_proposal_criterion_judgments; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_proposal_evidence; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_proposals; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_reviewer_decisions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_run_evidence; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_runs; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_screening_audit_labels; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_screening_cohort_members; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_screening_cohorts; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_screening_dispositions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_usage_backfill_state; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.ai_usage_backfill_state (singleton, completed_at, processed_rows, updated_at) VALUES (true, NULL, 0, CURRENT_TIMESTAMP);


--
-- Data for Name: ai_usage_corrections; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_usage_detached_projects; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_usage_fact_ids; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_usage_ledger; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_usage_legacy_backfill; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_usage_limit_changes; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_usage_limits; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.ai_usage_limits (scope_kind, scope_id, limit_micros, updated_at) VALUES ('platform', '00000000-0000-0000-0000-000000000001', 0, CURRENT_TIMESTAMP);


--
-- Data for Name: ai_usage_period_buckets; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.ai_usage_period_buckets (scope_kind, scope_id, workspace_id, project_id, run_budget_id, period_start, limit_micros, spent_micros, reserved_micros, updated_at) VALUES ('platform', '00000000-0000-0000-0000-000000000001', NULL, NULL, NULL, date_trunc('month', CURRENT_TIMESTAMP AT TIME ZONE 'UTC')::date, 0, 0, 0, CURRENT_TIMESTAMP);


--
-- Data for Name: ai_usage_reservations; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ai_usage_rollout_state; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.ai_usage_rollout_state (singleton, ai_hard_caps_enforced, last_legacy_ledger_insert_at, hard_caps_enabled_at, hard_caps_disabled_at, updated_at, ai_usage_export_activated, ai_usage_export_activated_at, ai_usage_export_deactivated_at) VALUES (true, false, NULL, NULL, NULL, CURRENT_TIMESTAMP, false, NULL, NULL);


--
-- Data for Name: ai_usage_workspace_admission_state; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.ai_usage_workspace_admission_state (workspace_id, blocked, updated_at) VALUES ('f45d8021-6271-556e-a59a-fd2e0a739f95', false, CURRENT_TIMESTAMP);


--
-- Data for Name: ai_usage_workspace_blocks; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: api_keys; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: appraisal_assessment_evidence; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: appraisal_assessments; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: appraisal_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: assistant_agent_runs; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: assistant_conversations; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: assistant_messages; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: assistant_plans; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: assistant_run_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: automation_definition_steps; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: automation_definitions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: automation_run_details; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: automation_step_details; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: billing_accounts; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: billing_free_enrollments; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: billing_inbox_invalidations; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: billing_usage_fact_targets; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: billing_usage_resolutions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: citations; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: crossref_raw_records; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: database_role_contract; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.database_role_contract (singleton, activated_at, activated_by) VALUES (true, NULL, NULL);


--
-- Data for Name: dedupe_proposals; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dedupe_resolution_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: document_block_embeddings; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: document_blocks; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: document_pages; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: document_references; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: document_sections; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: documents; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: durable_attempts; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: durable_runs; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: durable_steps; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: eligibility_criteria; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: exclusion_reasons; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: extraction_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: extraction_field_definitions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: extraction_values; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: fetched_citation_facts; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: fetched_unresolved_reference_facts; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: fulfilled_account_deletions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: integration_inbox_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: jobs; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: legacy_actor_mappings; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: legacy_actor_non_identities; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: metric_snapshots; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: notifications; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: object_lifecycle; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: object_retained_references; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: object_storage_rollout; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.object_storage_rollout (singleton, tenant_keys_activated, activated_at, parser_rollback_floor) VALUES (true, false, NULL, NULL);


--
-- Data for Name: operational_rate_limits; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: outbox_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: outbox_receipts; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: pmid_import_items; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: prisma_snapshots; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: project_access; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: project_ai_autonomy; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: project_ai_screening_authority; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: project_reports; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: project_unresolved_references; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: projection_state; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.projection_state (projection_name, project_id, state, revision, watermark, lag, last_success_at, last_error, rebuild_state, updated_at, generation, attempt) VALUES ('postgres_graph', NULL, 'pending', 0, 0, 0, NULL, NULL, NULL, CURRENT_TIMESTAMP, 0, 0);


--
-- Data for Name: projects; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: protocol_versions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: provider_rate_state; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: record_identifiers; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: records; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: report_fetch_state; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: report_identifiers; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: reports; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: review_artifact_lineage; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: review_artifacts; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: review_attempt_details; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: review_calibration_bundles; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: review_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: review_proposal_finalizations; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: review_run_manifests; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: scientific_assignments; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: screening_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: screening_state; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: second_review_attempts; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: second_review_gate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: settings; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: studies; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: study_events; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: study_reports; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: transport_dead_letters; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: user_sessions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: users; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: worker_heartbeats; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workflow_file_storage_state; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.workflow_file_storage_state (singleton, mode, changed_at) VALUES (true, 'overlap', CURRENT_TIMESTAMP);


--
-- Data for Name: workflow_files; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workflow_run_details; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workflow_secrets; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workflow_seen_items; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workflow_step_details; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workflow_versions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workflows; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: works; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workspace_invitations; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workspace_memberships; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workspaces; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.workspaces (id, kind, name, slug, workos_organization_id, created_at, updated_at, workos_reconciled_through) VALUES ('f45d8021-6271-556e-a59a-fd2e0a739f95', 'migration', 'Existing projects', 'existing-projects', NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, NULL);


--
-- Name: ai_concurrency_slots_generation_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.ai_concurrency_slots_generation_seq', 1, false);


--
-- Name: integration_inbox_events_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.integration_inbox_events_id_seq', 1, false);


--
-- Name: notification_revision_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.notification_revision_seq', 1, false);


--
-- Name: account_data_exports account_data_exports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_data_exports
    ADD CONSTRAINT account_data_exports_pkey PRIMARY KEY (id);


--
-- Name: account_deletion_requests account_deletion_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_deletion_requests
    ADD CONSTRAINT account_deletion_requests_pkey PRIMARY KEY (id);


--
-- Name: account_erasure_objects account_erasure_objects_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_erasure_objects
    ADD CONSTRAINT account_erasure_objects_pkey PRIMARY KEY (user_id, storage_key);


--
-- Name: account_erasure_progress account_erasure_progress_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_erasure_progress
    ADD CONSTRAINT account_erasure_progress_pkey PRIMARY KEY (user_id);


--
-- Name: account_erasure_rollout account_erasure_rollout_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_erasure_rollout
    ADD CONSTRAINT account_erasure_rollout_pkey PRIMARY KEY (singleton);


--
-- Name: account_erasure_writers account_erasure_writers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_erasure_writers
    ADD CONSTRAINT account_erasure_writers_pkey PRIMARY KEY (user_id, attempt_id);


--
-- Name: account_export_rollout account_export_rollout_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_export_rollout
    ADD CONSTRAINT account_export_rollout_pkey PRIMARY KEY (singleton);


--
-- Name: account_export_writers account_export_writers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_export_writers
    ADD CONSTRAINT account_export_writers_pkey PRIMARY KEY (attempt_id);


--
-- Name: account_human_activity account_human_activity_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_human_activity
    ADD CONSTRAINT account_human_activity_pkey PRIMARY KEY (user_id);


--
-- Name: acquisition_items acquisition_items_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.acquisition_items
    ADD CONSTRAINT acquisition_items_pkey PRIMARY KEY (acquisition_id, canonical_doi);


--
-- Name: acquisition_runs acquisition_runs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.acquisition_runs
    ADD CONSTRAINT acquisition_runs_pkey PRIMARY KEY (id);


--
-- Name: admin_audit_events admin_audit_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.admin_audit_events
    ADD CONSTRAINT admin_audit_events_pkey PRIMARY KEY (id);


--
-- Name: ai_activity ai_activity_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_activity
    ADD CONSTRAINT ai_activity_pkey PRIMARY KEY (id);


--
-- Name: ai_autonomy_outbox ai_autonomy_outbox_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_autonomy_outbox
    ADD CONSTRAINT ai_autonomy_outbox_pkey PRIMARY KEY (project_id, proposal_id);


--
-- Name: ai_concurrency_slots ai_concurrency_slots_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_concurrency_slots
    ADD CONSTRAINT ai_concurrency_slots_pkey PRIMARY KEY (scope_kind, scope_key, slot_index);


--
-- Name: ai_model_routes ai_model_routes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_model_routes
    ADD CONSTRAINT ai_model_routes_pkey PRIMARY KEY (id);


--
-- Name: ai_model_routes ai_model_routes_profile_provider_model_model_version_effect_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_model_routes
    ADD CONSTRAINT ai_model_routes_profile_provider_model_model_version_effect_key UNIQUE (profile, provider, model, model_version, effective_from);


--
-- Name: ai_opinion_exposures ai_opinion_exposures_audience_user_id_shape_check; Type: CHECK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE public.ai_opinion_exposures
    ADD CONSTRAINT ai_opinion_exposures_audience_user_id_shape_check CHECK (((audience_actor_user_id IS NULL) OR ((audience_actor_kind = 'user'::text) AND (audience_actor_id IS NOT NULL)))) NOT VALID;


--
-- Name: ai_opinion_exposures ai_opinion_exposures_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_opinion_exposures
    ADD CONSTRAINT ai_opinion_exposures_pkey PRIMARY KEY (id);


--
-- Name: ai_proposal_criterion_judgments ai_proposal_criterion_judgments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_criterion_judgments
    ADD CONSTRAINT ai_proposal_criterion_judgments_pkey PRIMARY KEY (proposal_id, criterion_id);


--
-- Name: ai_proposal_criterion_judgments ai_proposal_criterion_judgments_proposal_id_ordinal_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_criterion_judgments
    ADD CONSTRAINT ai_proposal_criterion_judgments_proposal_id_ordinal_key UNIQUE (proposal_id, ordinal);


--
-- Name: ai_proposal_evidence ai_proposal_evidence_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_evidence
    ADD CONSTRAINT ai_proposal_evidence_pkey PRIMARY KEY (proposal_id, ordinal);


--
-- Name: ai_proposals ai_proposals_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposals
    ADD CONSTRAINT ai_proposals_pkey PRIMARY KEY (id);


--
-- Name: ai_reviewer_decisions ai_reviewer_decisions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_reviewer_decisions
    ADD CONSTRAINT ai_reviewer_decisions_pkey PRIMARY KEY (id);


--
-- Name: ai_run_evidence ai_run_evidence_ai_run_id_rank_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_run_evidence
    ADD CONSTRAINT ai_run_evidence_ai_run_id_rank_key UNIQUE (ai_run_id, rank);


--
-- Name: ai_run_evidence ai_run_evidence_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_run_evidence
    ADD CONSTRAINT ai_run_evidence_pkey PRIMARY KEY (ai_run_id, document_block_id);


--
-- Name: ai_runs ai_runs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_runs
    ADD CONSTRAINT ai_runs_pkey PRIMARY KEY (id);


--
-- Name: ai_screening_audit_labels ai_screening_audit_labels_cohort_id_report_id_actor_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_audit_labels
    ADD CONSTRAINT ai_screening_audit_labels_cohort_id_report_id_actor_id_key UNIQUE (cohort_id, report_id, actor_id);


--
-- Name: ai_screening_audit_labels ai_screening_audit_labels_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_audit_labels
    ADD CONSTRAINT ai_screening_audit_labels_pkey PRIMARY KEY (id);


--
-- Name: ai_screening_cohort_members ai_screening_cohort_members_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohort_members
    ADD CONSTRAINT ai_screening_cohort_members_pkey PRIMARY KEY (cohort_id, report_id);


--
-- Name: ai_screening_cohorts ai_screening_cohorts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohorts
    ADD CONSTRAINT ai_screening_cohorts_pkey PRIMARY KEY (id);


--
-- Name: ai_screening_cohorts ai_screening_cohorts_project_id_audit_ordinal_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohorts
    ADD CONSTRAINT ai_screening_cohorts_project_id_audit_ordinal_key UNIQUE (project_id, audit_ordinal);


--
-- Name: ai_screening_cohorts ai_screening_cohorts_project_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohorts
    ADD CONSTRAINT ai_screening_cohorts_project_id_id_key UNIQUE (project_id, id);


--
-- Name: ai_screening_dispositions ai_screening_dispositions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_dispositions
    ADD CONSTRAINT ai_screening_dispositions_pkey PRIMARY KEY (id);


--
-- Name: ai_usage_backfill_state ai_usage_backfill_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_backfill_state
    ADD CONSTRAINT ai_usage_backfill_state_pkey PRIMARY KEY (singleton);


--
-- Name: ai_usage_corrections ai_usage_corrections_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_corrections
    ADD CONSTRAINT ai_usage_corrections_pkey PRIMARY KEY (id);


--
-- Name: ai_usage_detached_projects ai_usage_detached_projects_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_detached_projects
    ADD CONSTRAINT ai_usage_detached_projects_pkey PRIMARY KEY (project_id);


--
-- Name: ai_usage_fact_ids ai_usage_fact_ids_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_fact_ids
    ADD CONSTRAINT ai_usage_fact_ids_pkey PRIMARY KEY (fact_id);


--
-- Name: ai_usage_ledger ai_usage_ledger_period_start_present_check; Type: CHECK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE public.ai_usage_ledger
    ADD CONSTRAINT ai_usage_ledger_period_start_present_check CHECK ((period_start IS NOT NULL)) NOT VALID;


--
-- Name: ai_usage_ledger ai_usage_ledger_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_ledger
    ADD CONSTRAINT ai_usage_ledger_pkey PRIMARY KEY (id);


--
-- Name: ai_usage_legacy_backfill ai_usage_legacy_backfill_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_legacy_backfill
    ADD CONSTRAINT ai_usage_legacy_backfill_pkey PRIMARY KEY (ledger_id);


--
-- Name: ai_usage_limit_changes ai_usage_limit_changes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_limit_changes
    ADD CONSTRAINT ai_usage_limit_changes_pkey PRIMARY KEY (id);


--
-- Name: ai_usage_limits ai_usage_limits_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_limits
    ADD CONSTRAINT ai_usage_limits_pkey PRIMARY KEY (scope_kind, scope_id);


--
-- Name: ai_usage_period_buckets ai_usage_period_buckets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_period_buckets
    ADD CONSTRAINT ai_usage_period_buckets_pkey PRIMARY KEY (scope_kind, scope_id, period_start);


--
-- Name: ai_usage_reservations ai_usage_reservations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_reservations
    ADD CONSTRAINT ai_usage_reservations_pkey PRIMARY KEY (id);


--
-- Name: ai_usage_rollout_state ai_usage_rollout_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_rollout_state
    ADD CONSTRAINT ai_usage_rollout_state_pkey PRIMARY KEY (singleton);


--
-- Name: ai_usage_workspace_admission_state ai_usage_workspace_admission_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_workspace_admission_state
    ADD CONSTRAINT ai_usage_workspace_admission_state_pkey PRIMARY KEY (workspace_id);


--
-- Name: ai_usage_workspace_blocks ai_usage_workspace_blocks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage_workspace_blocks
    ADD CONSTRAINT ai_usage_workspace_blocks_pkey PRIMARY KEY (workspace_id);


--
-- Name: api_keys api_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_pkey PRIMARY KEY (id);


--
-- Name: appraisal_assessment_evidence appraisal_assessment_evidence_assessment_id_question_id_doc_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_assessment_evidence
    ADD CONSTRAINT appraisal_assessment_evidence_assessment_id_question_id_doc_key UNIQUE (assessment_id, question_id, document_id, block_id);


--
-- Name: appraisal_assessment_evidence appraisal_assessment_evidence_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_assessment_evidence
    ADD CONSTRAINT appraisal_assessment_evidence_pkey PRIMARY KEY (id);


--
-- Name: appraisal_assessments appraisal_assessments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_assessments
    ADD CONSTRAINT appraisal_assessments_pkey PRIMARY KEY (id);


--
-- Name: appraisal_assessments appraisal_assessments_project_id_report_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_assessments
    ADD CONSTRAINT appraisal_assessments_project_id_report_id_id_key UNIQUE (project_id, report_id, id);


--
-- Name: appraisal_events appraisal_events_assessment_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_events
    ADD CONSTRAINT appraisal_events_assessment_id_key UNIQUE (assessment_id);


--
-- Name: appraisal_events appraisal_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_events
    ADD CONSTRAINT appraisal_events_pkey PRIMARY KEY (id);


--
-- Name: assistant_agent_runs assistant_agent_runs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_agent_runs
    ADD CONSTRAINT assistant_agent_runs_pkey PRIMARY KEY (id);


--
-- Name: assistant_conversations assistant_conversations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_conversations
    ADD CONSTRAINT assistant_conversations_pkey PRIMARY KEY (id);


--
-- Name: assistant_messages assistant_messages_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_messages
    ADD CONSTRAINT assistant_messages_pkey PRIMARY KEY (id);


--
-- Name: assistant_plans assistant_plans_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_plans
    ADD CONSTRAINT assistant_plans_pkey PRIMARY KEY (id);


--
-- Name: assistant_run_events assistant_run_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_run_events
    ADD CONSTRAINT assistant_run_events_pkey PRIMARY KEY (run_id, seq);


--
-- Name: automation_definition_steps automation_definition_steps_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_definition_steps
    ADD CONSTRAINT automation_definition_steps_pkey PRIMARY KEY (project_id, definition_id, ordinal);


--
-- Name: automation_definition_steps automation_definition_steps_project_id_definition_id_step_k_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_definition_steps
    ADD CONSTRAINT automation_definition_steps_project_id_definition_id_step_k_key UNIQUE (project_id, definition_id, step_key);


--
-- Name: automation_definitions automation_definitions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_definitions
    ADD CONSTRAINT automation_definitions_pkey PRIMARY KEY (id);


--
-- Name: automation_definitions automation_definitions_project_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_definitions
    ADD CONSTRAINT automation_definitions_project_id_key UNIQUE (project_id, id);


--
-- Name: automation_definitions automation_definitions_project_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_definitions
    ADD CONSTRAINT automation_definitions_project_name_key UNIQUE (project_id, name);


--
-- Name: automation_run_details automation_run_details_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_run_details
    ADD CONSTRAINT automation_run_details_pkey PRIMARY KEY (id);


--
-- Name: automation_run_details automation_run_details_project_id_definition_id_trigger_kin_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_run_details
    ADD CONSTRAINT automation_run_details_project_id_definition_id_trigger_kin_key UNIQUE (project_id, definition_id, trigger_kind, idempotency_key);


--
-- Name: automation_step_details automation_step_details_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_step_details
    ADD CONSTRAINT automation_step_details_pkey PRIMARY KEY (id);


--
-- Name: automation_step_details automation_step_details_project_id_run_id_ordinal_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_step_details
    ADD CONSTRAINT automation_step_details_project_id_run_id_ordinal_key UNIQUE (project_id, run_id, ordinal);


--
-- Name: automation_step_details automation_step_details_project_id_run_id_step_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_step_details
    ADD CONSTRAINT automation_step_details_project_id_run_id_step_key_key UNIQUE (project_id, run_id, step_key);


--
-- Name: billing_accounts billing_accounts_operation_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_accounts
    ADD CONSTRAINT billing_accounts_operation_id_key UNIQUE (operation_id);


--
-- Name: billing_accounts billing_accounts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_accounts
    ADD CONSTRAINT billing_accounts_pkey PRIMARY KEY (id);


--
-- Name: billing_accounts billing_accounts_provider_scope_customer_ref_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_accounts
    ADD CONSTRAINT billing_accounts_provider_scope_customer_ref_key UNIQUE (provider_scope, customer_ref);


--
-- Name: billing_accounts billing_accounts_workspace_id_provider_scope_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_accounts
    ADD CONSTRAINT billing_accounts_workspace_id_provider_scope_key UNIQUE (workspace_id, provider_scope);


--
-- Name: billing_free_enrollments billing_free_enrollments_operation_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_free_enrollments
    ADD CONSTRAINT billing_free_enrollments_operation_id_key UNIQUE (operation_id);


--
-- Name: billing_free_enrollments billing_free_enrollments_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_free_enrollments
    ADD CONSTRAINT billing_free_enrollments_pkey PRIMARY KEY (workspace_id);


--
-- Name: billing_inbox_invalidations billing_inbox_invalidations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_inbox_invalidations
    ADD CONSTRAINT billing_inbox_invalidations_pkey PRIMARY KEY (inbox_id);


--
-- Name: billing_usage_fact_targets billing_usage_fact_targets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_usage_fact_targets
    ADD CONSTRAINT billing_usage_fact_targets_pkey PRIMARY KEY (fact_id);


--
-- Name: billing_usage_resolutions billing_usage_resolutions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_usage_resolutions
    ADD CONSTRAINT billing_usage_resolutions_pkey PRIMARY KEY (fact_id);


--
-- Name: billing_usage_resolutions billing_usage_resolutions_resolution_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_usage_resolutions
    ADD CONSTRAINT billing_usage_resolutions_resolution_id_key UNIQUE (resolution_id);


--
-- Name: citations citations_pkey1; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.citations
    ADD CONSTRAINT citations_pkey1 PRIMARY KEY (project_id, source_report_id, target_report_id);


--
-- Name: crossref_raw_records crossref_raw_records_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.crossref_raw_records
    ADD CONSTRAINT crossref_raw_records_pkey PRIMARY KEY (canonical_doi);


--
-- Name: database_role_contract database_role_contract_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.database_role_contract
    ADD CONSTRAINT database_role_contract_pkey PRIMARY KEY (singleton);


--
-- Name: dedupe_proposals dedupe_proposals_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_proposals
    ADD CONSTRAINT dedupe_proposals_pkey PRIMARY KEY (id);


--
-- Name: dedupe_proposals dedupe_proposals_project_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_proposals
    ADD CONSTRAINT dedupe_proposals_project_id_id_key UNIQUE (project_id, id);


--
-- Name: dedupe_resolution_events dedupe_resolution_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_resolution_events
    ADD CONSTRAINT dedupe_resolution_events_pkey PRIMARY KEY (id);


--
-- Name: dedupe_resolution_events dedupe_resolution_events_project_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_resolution_events
    ADD CONSTRAINT dedupe_resolution_events_project_id_id_key UNIQUE (project_id, id);


--
-- Name: dedupe_resolution_events dedupe_resolution_events_project_id_record_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_resolution_events
    ADD CONSTRAINT dedupe_resolution_events_project_id_record_id_id_key UNIQUE (project_id, record_id, id);


--
-- Name: document_block_embeddings document_block_embeddings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_block_embeddings
    ADD CONSTRAINT document_block_embeddings_pkey PRIMARY KEY (document_block_id, model_identifier, generation, content_hash);


--
-- Name: document_blocks document_blocks_document_id_id_parser_uq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_blocks
    ADD CONSTRAINT document_blocks_document_id_id_parser_uq UNIQUE (document_id, id, parser_version);


--
-- Name: document_blocks document_blocks_document_id_id_uq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_blocks
    ADD CONSTRAINT document_blocks_document_id_id_uq UNIQUE (document_id, id);


--
-- Name: document_blocks document_blocks_document_id_parser_version_ordinal_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_blocks
    ADD CONSTRAINT document_blocks_document_id_parser_version_ordinal_key UNIQUE (document_id, parser_version, ordinal);


--
-- Name: document_blocks document_blocks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_blocks
    ADD CONSTRAINT document_blocks_pkey PRIMARY KEY (id);


--
-- Name: document_pages document_pages_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_pages
    ADD CONSTRAINT document_pages_pkey PRIMARY KEY (document_id, parser_version, page_number);


--
-- Name: document_references document_references_document_id_parser_version_ordinal_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_references
    ADD CONSTRAINT document_references_document_id_parser_version_ordinal_key UNIQUE (document_id, parser_version, ordinal);


--
-- Name: document_references document_references_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_references
    ADD CONSTRAINT document_references_pkey PRIMARY KEY (id);


--
-- Name: document_sections document_sections_document_id_parser_version_ordinal_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_sections
    ADD CONSTRAINT document_sections_document_id_parser_version_ordinal_key UNIQUE (document_id, parser_version, ordinal);


--
-- Name: document_sections document_sections_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_sections
    ADD CONSTRAINT document_sections_pkey PRIMARY KEY (id);


--
-- Name: documents documents_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.documents
    ADD CONSTRAINT documents_pkey PRIMARY KEY (id);


--
-- Name: documents documents_project_id_id_uq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.documents
    ADD CONSTRAINT documents_project_id_id_uq UNIQUE (project_id, id);


--
-- Name: documents documents_project_report_id_uq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.documents
    ADD CONSTRAINT documents_project_report_id_uq UNIQUE (project_id, report_id, id);


--
-- Name: durable_attempts durable_attempts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_attempts
    ADD CONSTRAINT durable_attempts_pkey PRIMARY KEY (id);


--
-- Name: durable_attempts durable_attempts_project_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_attempts
    ADD CONSTRAINT durable_attempts_project_id_id_key UNIQUE (project_id, id);


--
-- Name: durable_attempts durable_attempts_project_id_run_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_attempts
    ADD CONSTRAINT durable_attempts_project_id_run_id_id_key UNIQUE (project_id, run_id, id);


--
-- Name: durable_attempts durable_attempts_step_id_attempt_number_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_attempts
    ADD CONSTRAINT durable_attempts_step_id_attempt_number_key UNIQUE (step_id, attempt_number);


--
-- Name: durable_runs durable_runs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_runs
    ADD CONSTRAINT durable_runs_pkey PRIMARY KEY (id);


--
-- Name: durable_runs durable_runs_project_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_runs
    ADD CONSTRAINT durable_runs_project_id_id_key UNIQUE (project_id, id);


--
-- Name: durable_steps durable_steps_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_steps
    ADD CONSTRAINT durable_steps_pkey PRIMARY KEY (id);


--
-- Name: durable_steps durable_steps_project_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_steps
    ADD CONSTRAINT durable_steps_project_id_id_key UNIQUE (project_id, id);


--
-- Name: durable_steps durable_steps_project_id_run_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_steps
    ADD CONSTRAINT durable_steps_project_id_run_id_id_key UNIQUE (project_id, run_id, id);


--
-- Name: durable_steps durable_steps_run_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_steps
    ADD CONSTRAINT durable_steps_run_id_id_key UNIQUE (run_id, id);


--
-- Name: eligibility_criteria eligibility_criteria_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.eligibility_criteria
    ADD CONSTRAINT eligibility_criteria_pkey PRIMARY KEY (id);


--
-- Name: eligibility_criteria eligibility_criteria_protocol_version_id_ordinal_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.eligibility_criteria
    ADD CONSTRAINT eligibility_criteria_protocol_version_id_ordinal_key UNIQUE (protocol_version_id, ordinal);


--
-- Name: exclusion_reasons exclusion_reasons_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.exclusion_reasons
    ADD CONSTRAINT exclusion_reasons_pkey PRIMARY KEY (id);


--
-- Name: exclusion_reasons exclusion_reasons_project_id_code_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.exclusion_reasons
    ADD CONSTRAINT exclusion_reasons_project_id_code_key UNIQUE (project_id, code);


--
-- Name: exclusion_reasons exclusion_reasons_project_id_id_stage_uq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.exclusion_reasons
    ADD CONSTRAINT exclusion_reasons_project_id_id_stage_uq UNIQUE (project_id, id, stage);


--
-- Name: extraction_events extraction_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_events
    ADD CONSTRAINT extraction_events_pkey PRIMARY KEY (id);


--
-- Name: extraction_events extraction_events_proposal_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_events
    ADD CONSTRAINT extraction_events_proposal_id_key UNIQUE (proposal_id);


--
-- Name: extraction_field_definitions extraction_field_definitions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_field_definitions
    ADD CONSTRAINT extraction_field_definitions_pkey PRIMARY KEY (project_id, id, version);


--
-- Name: extraction_field_definitions extraction_field_definitions_project_id_field_key_version_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_field_definitions
    ADD CONSTRAINT extraction_field_definitions_project_id_field_key_version_key UNIQUE (project_id, field_key, version);


--
-- Name: extraction_values extraction_values_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_values
    ADD CONSTRAINT extraction_values_pkey PRIMARY KEY (id);


--
-- Name: fetched_citation_facts fetched_citation_facts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fetched_citation_facts
    ADD CONSTRAINT fetched_citation_facts_pkey PRIMARY KEY (source_doi, target_doi);


--
-- Name: fetched_unresolved_reference_facts fetched_unresolved_reference_facts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fetched_unresolved_reference_facts
    ADD CONSTRAINT fetched_unresolved_reference_facts_pkey PRIMARY KEY (id);


--
-- Name: fulfilled_account_deletions fulfilled_account_deletions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fulfilled_account_deletions
    ADD CONSTRAINT fulfilled_account_deletions_pkey PRIMARY KEY (user_id);


--
-- Name: integration_inbox_events integration_inbox_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.integration_inbox_events
    ADD CONSTRAINT integration_inbox_events_pkey PRIMARY KEY (id);


--
-- Name: integration_inbox_events integration_inbox_events_provider_provider_event_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.integration_inbox_events
    ADD CONSTRAINT integration_inbox_events_provider_provider_event_id_key UNIQUE (provider, provider_event_id);


--
-- Name: jobs jobs_dedupe_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.jobs
    ADD CONSTRAINT jobs_dedupe_key_key UNIQUE (dedupe_key);


--
-- Name: jobs jobs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.jobs
    ADD CONSTRAINT jobs_pkey PRIMARY KEY (id);


--
-- Name: jobs jobs_project_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.jobs
    ADD CONSTRAINT jobs_project_id_id_key UNIQUE (project_id, id);


--
-- Name: jobs jobs_scope_check; Type: CHECK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE public.jobs
    ADD CONSTRAINT jobs_scope_check CHECK ((((scope = 'project'::text) AND (workspace_id IS NULL) AND ((project_id IS NOT NULL) OR (state = ANY (ARRAY['completed'::text, 'failed'::text, 'dead'::text])))) OR ((scope = 'workspace'::text) AND (project_id IS NULL) AND (workspace_id IS NOT NULL)) OR ((scope = 'platform'::text) AND (project_id IS NULL) AND (workspace_id IS NULL)))) NOT VALID;


--
-- Name: legacy_actor_mappings legacy_actor_mappings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legacy_actor_mappings
    ADD CONSTRAINT legacy_actor_mappings_pkey PRIMARY KEY (legacy_actor_kind, legacy_actor_id);


--
-- Name: legacy_actor_non_identities legacy_actor_non_identities_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legacy_actor_non_identities
    ADD CONSTRAINT legacy_actor_non_identities_pkey PRIMARY KEY (legacy_actor_kind, legacy_actor_id, user_id);


--
-- Name: metric_snapshots metric_snapshots_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.metric_snapshots
    ADD CONSTRAINT metric_snapshots_pkey PRIMARY KEY (project_id, revision);


--
-- Name: notifications notifications_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_pkey PRIMARY KEY (id);


--
-- Name: object_lifecycle object_lifecycle_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.object_lifecycle
    ADD CONSTRAINT object_lifecycle_pkey PRIMARY KEY (storage_key);


--
-- Name: object_retained_references object_retained_references_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.object_retained_references
    ADD CONSTRAINT object_retained_references_pkey PRIMARY KEY (reference_id, storage_key);


--
-- Name: object_storage_rollout object_storage_rollout_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.object_storage_rollout
    ADD CONSTRAINT object_storage_rollout_pkey PRIMARY KEY (singleton);


--
-- Name: operational_rate_limits operational_rate_limits_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.operational_rate_limits
    ADD CONSTRAINT operational_rate_limits_pkey PRIMARY KEY (subject_kind, subject_id);


--
-- Name: outbox_events outbox_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.outbox_events
    ADD CONSTRAINT outbox_events_pkey PRIMARY KEY (event_id);


--
-- Name: outbox_receipts outbox_receipts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.outbox_receipts
    ADD CONSTRAINT outbox_receipts_pkey PRIMARY KEY (event_id, consumer);


--
-- Name: pmid_import_items pmid_import_items_acquisition_run_id_position_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.pmid_import_items
    ADD CONSTRAINT pmid_import_items_acquisition_run_id_position_key UNIQUE (acquisition_run_id, "position");


--
-- Name: pmid_import_items pmid_import_items_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.pmid_import_items
    ADD CONSTRAINT pmid_import_items_pkey PRIMARY KEY (acquisition_run_id, pmid);


--
-- Name: prisma_snapshots prisma_snapshots_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.prisma_snapshots
    ADD CONSTRAINT prisma_snapshots_pkey PRIMARY KEY (project_id);


--
-- Name: project_access project_access_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_access
    ADD CONSTRAINT project_access_pkey PRIMARY KEY (id);


--
-- Name: project_access project_access_project_id_user_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_access
    ADD CONSTRAINT project_access_project_id_user_id_key UNIQUE (project_id, user_id);


--
-- Name: project_ai_autonomy project_ai_autonomy_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_ai_autonomy
    ADD CONSTRAINT project_ai_autonomy_pkey PRIMARY KEY (project_id, task);


--
-- Name: project_ai_screening_authority project_ai_screening_authority_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_ai_screening_authority
    ADD CONSTRAINT project_ai_screening_authority_pkey PRIMARY KEY (project_id);


--
-- Name: project_reports project_reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_reports
    ADD CONSTRAINT project_reports_pkey PRIMARY KEY (project_id, report_id);


--
-- Name: project_unresolved_references project_unresolved_references_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_unresolved_references
    ADD CONSTRAINT project_unresolved_references_pkey PRIMARY KEY (project_id, id);


--
-- Name: projects projects_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_pkey PRIMARY KEY (id);


--
-- Name: protocol_versions protocol_versions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.protocol_versions
    ADD CONSTRAINT protocol_versions_pkey PRIMARY KEY (id);


--
-- Name: protocol_versions protocol_versions_project_id_id_unique; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.protocol_versions
    ADD CONSTRAINT protocol_versions_project_id_id_unique UNIQUE (project_id, id);


--
-- Name: protocol_versions protocol_versions_project_id_version_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.protocol_versions
    ADD CONSTRAINT protocol_versions_project_id_version_key UNIQUE (project_id, version);


--
-- Name: provider_rate_state provider_rate_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.provider_rate_state
    ADD CONSTRAINT provider_rate_state_pkey PRIMARY KEY (provider);


--
-- Name: record_identifiers record_identifiers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.record_identifiers
    ADD CONSTRAINT record_identifiers_pkey PRIMARY KEY (id);


--
-- Name: record_identifiers record_identifiers_record_id_scheme_normalized_value_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.record_identifiers
    ADD CONSTRAINT record_identifiers_record_id_scheme_normalized_value_key UNIQUE (record_id, scheme, normalized_value);


--
-- Name: records records_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.records
    ADD CONSTRAINT records_pkey PRIMARY KEY (id);


--
-- Name: records records_project_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.records
    ADD CONSTRAINT records_project_id_id_key UNIQUE (project_id, id);


--
-- Name: records records_project_id_source_source_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.records
    ADD CONSTRAINT records_project_id_source_source_key_key UNIQUE (project_id, source, source_key);


--
-- Name: report_fetch_state report_fetch_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_fetch_state
    ADD CONSTRAINT report_fetch_state_pkey PRIMARY KEY (report_id);


--
-- Name: report_identifiers report_identifiers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_identifiers
    ADD CONSTRAINT report_identifiers_pkey PRIMARY KEY (id);


--
-- Name: report_identifiers report_identifiers_scheme_normalized_value_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_identifiers
    ADD CONSTRAINT report_identifiers_scheme_normalized_value_key UNIQUE (scheme, normalized_value);


--
-- Name: reports reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_pkey PRIMARY KEY (id);


--
-- Name: review_artifact_lineage review_artifact_lineage_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_artifact_lineage
    ADD CONSTRAINT review_artifact_lineage_pkey PRIMARY KEY (project_id, artifact_id, predecessor_artifact_id);


--
-- Name: review_artifacts review_artifacts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_artifacts
    ADD CONSTRAINT review_artifacts_pkey PRIMARY KEY (id);


--
-- Name: review_artifacts review_artifacts_project_id_content_hash_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_artifacts
    ADD CONSTRAINT review_artifacts_project_id_content_hash_key UNIQUE (project_id, content_hash);


--
-- Name: review_artifacts review_artifacts_project_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_artifacts
    ADD CONSTRAINT review_artifacts_project_id_id_key UNIQUE (project_id, id);


--
-- Name: review_attempt_details review_attempt_details_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_attempt_details
    ADD CONSTRAINT review_attempt_details_pkey PRIMARY KEY (id);


--
-- Name: review_calibration_bundles review_calibration_bundles_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_calibration_bundles
    ADD CONSTRAINT review_calibration_bundles_pkey PRIMARY KEY (id);


--
-- Name: review_calibration_bundles review_calibration_bundles_project_id_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_calibration_bundles
    ADD CONSTRAINT review_calibration_bundles_project_id_id_key UNIQUE (project_id, id);


--
-- Name: review_events review_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_events
    ADD CONSTRAINT review_events_pkey PRIMARY KEY (id);


--
-- Name: review_proposal_finalizations review_proposal_finalizations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_proposal_finalizations
    ADD CONSTRAINT review_proposal_finalizations_pkey PRIMARY KEY (project_id, automation_run_id, candidate_hash);


--
-- Name: review_proposal_finalizations review_proposal_finalizations_proposal_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_proposal_finalizations
    ADD CONSTRAINT review_proposal_finalizations_proposal_id_key UNIQUE (proposal_id);


--
-- Name: review_run_manifests review_run_manifests_automation_run_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_run_manifests
    ADD CONSTRAINT review_run_manifests_automation_run_id_key UNIQUE (automation_run_id);


--
-- Name: review_run_manifests review_run_manifests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_run_manifests
    ADD CONSTRAINT review_run_manifests_pkey PRIMARY KEY (project_id, automation_run_id);


--
-- Name: screening_events screening_events_actor_user_id_kind_check; Type: CHECK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE public.screening_events
    ADD CONSTRAINT screening_events_actor_user_id_kind_check CHECK (((actor_user_id IS NULL) OR (actor_kind = 'user'::text))) NOT VALID;


--
-- Name: screening_events screening_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_events
    ADD CONSTRAINT screening_events_pkey PRIMARY KEY (id);


--
-- Name: screening_state screening_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_state
    ADD CONSTRAINT screening_state_pkey PRIMARY KEY (project_id, report_id);


--
-- Name: second_review_attempts second_review_attempts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.second_review_attempts
    ADD CONSTRAINT second_review_attempts_pkey PRIMARY KEY (project_id, report_id, stage, protocol_version_id);


--
-- Name: second_review_gate second_review_gate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.second_review_gate
    ADD CONSTRAINT second_review_gate_pkey PRIMARY KEY (project_id, stage);


--
-- Name: settings settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.settings
    ADD CONSTRAINT settings_pkey PRIMARY KEY (id);


--
-- Name: studies studies_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.studies
    ADD CONSTRAINT studies_pkey PRIMARY KEY (id);


--
-- Name: studies studies_project_id_id_uq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.studies
    ADD CONSTRAINT studies_project_id_id_uq UNIQUE (project_id, id);


--
-- Name: study_events study_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_events
    ADD CONSTRAINT study_events_pkey PRIMARY KEY (id);


--
-- Name: study_reports study_reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_reports
    ADD CONSTRAINT study_reports_pkey PRIMARY KEY (project_id, study_id, report_id);


--
-- Name: transport_dead_letters transport_dead_letters_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.transport_dead_letters
    ADD CONSTRAINT transport_dead_letters_pkey PRIMARY KEY (delivery_id);


--
-- Name: user_sessions user_sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_pkey PRIMARY KEY (id);


--
-- Name: user_sessions user_sessions_session_token_hash_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_session_token_hash_key UNIQUE (session_token_hash);


--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);


--
-- Name: users users_workos_user_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_workos_user_id_key UNIQUE (workos_user_id);


--
-- Name: worker_heartbeats worker_heartbeats_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.worker_heartbeats
    ADD CONSTRAINT worker_heartbeats_pkey PRIMARY KEY (owner);


--
-- Name: workflow_file_storage_state workflow_file_storage_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_file_storage_state
    ADD CONSTRAINT workflow_file_storage_state_pkey PRIMARY KEY (singleton);


--
-- Name: workflow_files workflow_files_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_files
    ADD CONSTRAINT workflow_files_pkey PRIMARY KEY (id);


--
-- Name: workflow_run_details workflow_run_details_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_run_details
    ADD CONSTRAINT workflow_run_details_pkey PRIMARY KEY (id);


--
-- Name: workflow_run_details workflow_run_details_workflow_id_idempotency_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_run_details
    ADD CONSTRAINT workflow_run_details_workflow_id_idempotency_key_key UNIQUE (workflow_id, idempotency_key);


--
-- Name: workflow_secrets workflow_secrets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_secrets
    ADD CONSTRAINT workflow_secrets_pkey PRIMARY KEY (workflow_id, node_id, key);


--
-- Name: workflow_seen_items workflow_seen_items_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_seen_items
    ADD CONSTRAINT workflow_seen_items_pkey PRIMARY KEY (workflow_id, identifier);


--
-- Name: workflow_step_details workflow_step_details_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_step_details
    ADD CONSTRAINT workflow_step_details_pkey PRIMARY KEY (id);


--
-- Name: workflow_step_details workflow_step_details_run_id_node_id_iteration_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_step_details
    ADD CONSTRAINT workflow_step_details_run_id_node_id_iteration_key UNIQUE (run_id, node_id, iteration);


--
-- Name: workflow_versions workflow_versions_number_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_versions
    ADD CONSTRAINT workflow_versions_number_key UNIQUE (workflow_id, version);


--
-- Name: workflow_versions workflow_versions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_versions
    ADD CONSTRAINT workflow_versions_pkey PRIMARY KEY (id);


--
-- Name: workflow_versions workflow_versions_workflow_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_versions
    ADD CONSTRAINT workflow_versions_workflow_id_key UNIQUE (workflow_id, id);


--
-- Name: workflows workflows_email_token_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflows
    ADD CONSTRAINT workflows_email_token_key UNIQUE (email_token);


--
-- Name: workflows workflows_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflows
    ADD CONSTRAINT workflows_pkey PRIMARY KEY (id);


--
-- Name: workflows workflows_project_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflows
    ADD CONSTRAINT workflows_project_id_key UNIQUE (project_id, id);


--
-- Name: workflows workflows_project_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflows
    ADD CONSTRAINT workflows_project_name_key UNIQUE (project_id, name);


--
-- Name: workflows workflows_webhook_token_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflows
    ADD CONSTRAINT workflows_webhook_token_key UNIQUE (webhook_token);


--
-- Name: works works_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.works
    ADD CONSTRAINT works_pkey PRIMARY KEY (canonical_doi);


--
-- Name: workspace_invitations workspace_invitations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_invitations
    ADD CONSTRAINT workspace_invitations_pkey PRIMARY KEY (id);


--
-- Name: workspace_memberships workspace_memberships_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_memberships
    ADD CONSTRAINT workspace_memberships_pkey PRIMARY KEY (id);


--
-- Name: workspace_memberships workspace_memberships_workos_membership_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_memberships
    ADD CONSTRAINT workspace_memberships_workos_membership_id_key UNIQUE (workos_membership_id);


--
-- Name: workspace_memberships workspace_memberships_workspace_id_user_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_memberships
    ADD CONSTRAINT workspace_memberships_workspace_id_user_id_key UNIQUE (workspace_id, user_id);


--
-- Name: workspaces workspaces_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_pkey PRIMARY KEY (id);


--
-- Name: workspaces workspaces_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_slug_key UNIQUE (slug);


--
-- Name: workspaces workspaces_workos_organization_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_workos_organization_id_key UNIQUE (workos_organization_id);


--
-- Name: account_data_exports_one_active; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX account_data_exports_one_active ON public.account_data_exports USING btree (user_id) WHERE (status = ANY (ARRAY['requested'::text, 'processing'::text]));


--
-- Name: account_data_exports_retained_until; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX account_data_exports_retained_until ON public.account_data_exports USING btree (retained_until) WHERE (status = 'completed'::text);


--
-- Name: account_data_exports_user_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX account_data_exports_user_status ON public.account_data_exports USING btree (user_id, status, requested_at DESC);


--
-- Name: account_deletion_requests_due_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX account_deletion_requests_due_idx ON public.account_deletion_requests USING btree (requested_at) WHERE (state = 'requested'::text);


--
-- Name: account_deletion_requests_open_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX account_deletion_requests_open_idx ON public.account_deletion_requests USING btree (user_id) WHERE (state = ANY (ARRAY['requested'::text, 'completing'::text]));


--
-- Name: account_export_writers_privacy_pending; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX account_export_writers_privacy_pending ON public.account_export_writers USING btree (privacy_due_at, attempt_id) WHERE ((stopped_at IS NULL) AND (privacy_due_at IS NOT NULL));


--
-- Name: acquisition_items_nonterminal_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX acquisition_items_nonterminal_idx ON public.acquisition_items USING btree (acquisition_id, queued_at) WHERE (status = ANY (ARRAY['queued'::text, 'fetching'::text]));


--
-- Name: acquisition_items_status_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX acquisition_items_status_idx ON public.acquisition_items USING btree (acquisition_id, status);


--
-- Name: acquisition_runs_project_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX acquisition_runs_project_created_idx ON public.acquisition_runs USING btree (project_id, created_at DESC, id DESC);


--
-- Name: acquisition_runs_project_idempotency_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX acquisition_runs_project_idempotency_idx ON public.acquisition_runs USING btree (project_id, idempotency_key) WHERE (idempotency_key IS NOT NULL);


--
-- Name: admin_audit_auth_rejection_window_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX admin_audit_auth_rejection_window_idx ON public.admin_audit_events USING btree (((metadata ->> 'stage'::text)), ((metadata ->> 'reason'::text)), COALESCE(target_user_id, '00000000-0000-0000-0000-000000000000'::uuid), ((metadata ->> 'window'::text))) WHERE (event_type = 'auth.rejected'::text);


--
-- Name: admin_audit_denial_window_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX admin_audit_denial_window_idx ON public.admin_audit_events USING btree (principal_user_id, ((metadata ->> 'permission'::text)), ((metadata ->> 'resource_type'::text)), ((metadata ->> 'window'::text))) WHERE (event_type = 'authorization.denied'::text);


--
-- Name: admin_audit_events_event_type_time_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX admin_audit_events_event_type_time_idx ON public.admin_audit_events USING btree (event_type, occurred_at DESC, id DESC);


--
-- Name: admin_audit_events_occurred_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX admin_audit_events_occurred_idx ON public.admin_audit_events USING btree (occurred_at DESC, id);


--
-- Name: admin_audit_events_principal_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX admin_audit_events_principal_idx ON public.admin_audit_events USING btree (principal_user_id, occurred_at DESC) WHERE (principal_user_id IS NOT NULL);


--
-- Name: admin_audit_events_target_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX admin_audit_events_target_idx ON public.admin_audit_events USING btree (target_user_id, occurred_at DESC) WHERE (target_user_id IS NOT NULL);


--
-- Name: admin_audit_events_workspace_time_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX admin_audit_events_workspace_time_idx ON public.admin_audit_events USING btree (workspace_id, occurred_at DESC, id DESC) WHERE (workspace_id IS NOT NULL);


--
-- Name: ai_activity_batch_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_activity_batch_idx ON public.ai_activity USING btree (project_id, batch_id) WHERE (batch_id IS NOT NULL);


--
-- Name: ai_activity_project_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_activity_project_idx ON public.ai_activity USING btree (project_id, created_at DESC, id DESC);


--
-- Name: ai_activity_task_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_activity_task_idx ON public.ai_activity USING btree (project_id, task, created_at DESC);


--
-- Name: ai_autonomy_outbox_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_autonomy_outbox_created_idx ON public.ai_autonomy_outbox USING btree (created_at);


--
-- Name: ai_concurrency_slots_expiry_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_concurrency_slots_expiry_idx ON public.ai_concurrency_slots USING btree (lease_expires_at);


--
-- Name: ai_first_active_disposition; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_first_active_disposition ON public.ai_screening_dispositions USING btree (project_id, report_id) WHERE ((voided_at IS NULL) AND (finalized_at IS NULL));


--
-- Name: ai_first_one_active; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_first_one_active ON public.ai_screening_cohorts USING btree (project_id) WHERE (status = ANY (ARRAY['open'::text, 'closed'::text, 'auditing'::text, 'passed'::text]));


--
-- Name: ai_model_routes_resolution_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_model_routes_resolution_idx ON public.ai_model_routes USING btree (profile, enabled, effective_from DESC, id);


--
-- Name: ai_opinion_exposures_decision_source_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_opinion_exposures_decision_source_uq ON public.ai_opinion_exposures USING btree (ai_reviewer_decision_id, exposure_source, COALESCE(audience_actor_kind, ''::text), COALESCE(audience_actor_id, ''::text)) WHERE (ai_reviewer_decision_id IS NOT NULL);


--
-- Name: ai_opinion_exposures_lookup_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_opinion_exposures_lookup_idx ON public.ai_opinion_exposures USING btree (project_id, report_id, stage, exposure_possible_at);


--
-- Name: ai_opinion_exposures_proposal_source_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_opinion_exposures_proposal_source_uq ON public.ai_opinion_exposures USING btree (proposal_id, exposure_source, COALESCE(audience_actor_kind, ''::text), COALESCE(audience_actor_id, ''::text)) WHERE ((proposal_id IS NOT NULL) AND (ai_reviewer_decision_id IS NULL));


--
-- Name: ai_proposal_criterion_project_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_proposal_criterion_project_idx ON public.ai_proposal_criterion_judgments USING btree (project_id, proposal_id, ordinal);


--
-- Name: ai_proposal_evidence_project_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_proposal_evidence_project_idx ON public.ai_proposal_evidence USING btree (project_id, proposal_id, ordinal);


--
-- Name: ai_proposals_entity_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_proposals_entity_idx ON public.ai_proposals USING btree (project_id, entity_type, entity_id, created_at DESC, id);


--
-- Name: ai_proposals_model_run_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_proposals_model_run_uq ON public.ai_proposals USING btree (model_run_id);


--
-- Name: ai_proposals_project_id_id_protocol_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_proposals_project_id_id_protocol_uq ON public.ai_proposals USING btree (project_id, id, protocol_version_id);


--
-- Name: ai_proposals_project_id_id_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_proposals_project_id_id_uq ON public.ai_proposals USING btree (project_id, id);


--
-- Name: ai_proposals_project_status_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_proposals_project_status_idx ON public.ai_proposals USING btree (project_id, status, created_at DESC, id);


--
-- Name: ai_proposals_project_study_target_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_proposals_project_study_target_idx ON public.ai_proposals USING btree (project_id, target_study_id, created_at DESC, id DESC);


--
-- Name: ai_proposals_project_target_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_proposals_project_target_idx ON public.ai_proposals USING btree (project_id, target_report_id, target_record_id, created_at DESC, id DESC);


--
-- Name: ai_proposals_project_task_status_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_proposals_project_task_status_idx ON public.ai_proposals USING btree (project_id, task_kind, status, created_at DESC, id DESC);


--
-- Name: ai_reviewer_decisions_current_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_reviewer_decisions_current_uq ON public.ai_reviewer_decisions USING btree (project_id, report_id, stage) WHERE (voided_at IS NULL);


--
-- Name: ai_reviewer_decisions_project_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_reviewer_decisions_project_idx ON public.ai_reviewer_decisions USING btree (project_id, stage, created_at DESC);


--
-- Name: ai_reviewer_decisions_proposal_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_reviewer_decisions_proposal_idx ON public.ai_reviewer_decisions USING btree (project_id, proposal_id) WHERE (proposal_id IS NOT NULL);


--
-- Name: ai_run_evidence_block_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_run_evidence_block_idx ON public.ai_run_evidence USING btree (document_id, document_block_id, ai_run_id);


--
-- Name: ai_runs_id_project_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_runs_id_project_uq ON public.ai_runs USING btree (id, project_id);


--
-- Name: ai_runs_parent_automation_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_runs_parent_automation_idx ON public.ai_runs USING btree (parent_automation_run_id, project_id);


--
-- Name: ai_runs_project_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_runs_project_created_idx ON public.ai_runs USING btree (project_id, created_at DESC, id);


--
-- Name: ai_runs_reuse_completed_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_runs_reuse_completed_idx ON public.ai_runs USING btree (reuse_hash, completed_at DESC, created_at DESC, id) WHERE (status = 'completed'::text);


--
-- Name: ai_runs_task_status_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_runs_task_status_idx ON public.ai_runs USING btree (task_kind, status, created_at DESC, id);


--
-- Name: ai_screening_audit_labels_actor_user_id_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_screening_audit_labels_actor_user_id_uq ON public.ai_screening_audit_labels USING btree (cohort_id, report_id, actor_user_id) WHERE (actor_user_id IS NOT NULL);


--
-- Name: ai_usage_corrections_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_usage_corrections_created_idx ON public.ai_usage_corrections USING btree (created_at, id);


--
-- Name: ai_usage_ledger_project_month_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_usage_ledger_project_month_idx ON public.ai_usage_ledger USING btree (project_id, created_at DESC);


--
-- Name: ai_usage_ledger_reservation_unique_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ai_usage_ledger_reservation_unique_idx ON public.ai_usage_ledger USING btree (reservation_id) WHERE (reservation_id IS NOT NULL);


--
-- Name: ai_usage_ledger_settle_keyset_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_usage_ledger_settle_keyset_idx ON public.ai_usage_ledger USING btree (created_at, id);


--
-- Name: ai_usage_reservations_job_fence_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_usage_reservations_job_fence_idx ON public.ai_usage_reservations USING btree (job_id, attempt_fence) WHERE ((job_id IS NOT NULL) AND (state = ANY (ARRAY['open'::text, 'observed'::text, 'unresolved'::text])));


--
-- Name: ai_usage_reservations_workspace_open_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_usage_reservations_workspace_open_idx ON public.ai_usage_reservations USING btree (workspace_id, opened_at) WHERE (state = ANY (ARRAY['open'::text, 'observed'::text, 'unresolved'::text]));


--
-- Name: api_keys_creator_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX api_keys_creator_idx ON public.api_keys USING btree (created_by_user_id);


--
-- Name: api_keys_workspace_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX api_keys_workspace_created_idx ON public.api_keys USING btree (workspace_id, created_at DESC, id);


--
-- Name: appraisal_assessments_project_report_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX appraisal_assessments_project_report_created_idx ON public.appraisal_assessments USING btree (project_id, report_id, created_at DESC, id);


--
-- Name: appraisal_events_project_report_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX appraisal_events_project_report_created_idx ON public.appraisal_events USING btree (project_id, report_id, created_at DESC, id);


--
-- Name: appraisal_evidence_project_report_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX appraisal_evidence_project_report_idx ON public.appraisal_assessment_evidence USING btree (project_id, report_id, assessment_id);


--
-- Name: assistant_agent_runs_conversation_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX assistant_agent_runs_conversation_idx ON public.assistant_agent_runs USING btree (conversation_id, created_at DESC);


--
-- Name: assistant_conversations_project_updated_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX assistant_conversations_project_updated_idx ON public.assistant_conversations USING btree (project_id, updated_at DESC);


--
-- Name: assistant_messages_conversation_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX assistant_messages_conversation_created_idx ON public.assistant_messages USING btree (conversation_id, created_at);


--
-- Name: assistant_plans_conversation_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX assistant_plans_conversation_idx ON public.assistant_plans USING btree (conversation_id, created_at DESC);


--
-- Name: assistant_plans_project_status_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX assistant_plans_project_status_idx ON public.assistant_plans USING btree (project_id, status, created_at DESC);


--
-- Name: automation_definitions_project_trigger_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX automation_definitions_project_trigger_idx ON public.automation_definitions USING btree (project_id, trigger_kind, status, name);


--
-- Name: billing_accounts_due; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX billing_accounts_due ON public.billing_accounts USING btree (reconcile_after, id) WHERE (provisioning <> 'detached'::text);


--
-- Name: billing_usage_fact_targets_workspace_scope_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX billing_usage_fact_targets_workspace_scope_idx ON public.billing_usage_fact_targets USING btree (workspace_id, provider_scope);


--
-- Name: billing_usage_resolutions_workspace_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX billing_usage_resolutions_workspace_idx ON public.billing_usage_resolutions USING btree (workspace_id);


--
-- Name: citations_project_source_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX citations_project_source_idx ON public.citations USING btree (project_id, source_report_id, target_report_id);


--
-- Name: citations_project_target_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX citations_project_target_idx ON public.citations USING btree (project_id, target_report_id);


--
-- Name: dedupe_pending_proposal_unique_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dedupe_pending_proposal_unique_idx ON public.dedupe_proposals USING btree (project_id, record_id, COALESCE(candidate_report_id, '00000000-0000-0000-0000-000000000000'::uuid), proposal_kind) WHERE (status = 'pending'::text);


--
-- Name: dedupe_proposals_project_status_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dedupe_proposals_project_status_idx ON public.dedupe_proposals USING btree (project_id, status, created_at DESC, id DESC);


--
-- Name: dedupe_resolution_events_record_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dedupe_resolution_events_record_idx ON public.dedupe_resolution_events USING btree (project_id, record_id, created_at DESC, id DESC);


--
-- Name: document_block_embeddings_current_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX document_block_embeddings_current_uq ON public.document_block_embeddings USING btree (document_block_id) WHERE is_current;


--
-- Name: document_block_embeddings_hnsw_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX document_block_embeddings_hnsw_idx ON public.document_block_embeddings USING hnsw (((embedding)::public.vector(1536)) public.vector_cosine_ops) WHERE (is_current AND (dimension = 1536));


--
-- Name: document_block_embeddings_lookup_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX document_block_embeddings_lookup_idx ON public.document_block_embeddings USING btree (document_block_id, content_hash, is_current, created_at DESC);


--
-- Name: document_blocks_active_ordinal_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX document_blocks_active_ordinal_uq ON public.document_blocks USING btree (document_id, ordinal) WHERE active;


--
-- Name: document_blocks_document_version_page_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX document_blocks_document_version_page_idx ON public.document_blocks USING btree (document_id, parser_version, page_number, ordinal);


--
-- Name: document_blocks_fts_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX document_blocks_fts_idx ON public.document_blocks USING gin (search_vector);


--
-- Name: document_pages_active_page_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX document_pages_active_page_uq ON public.document_pages USING btree (document_id, page_number) WHERE active;


--
-- Name: document_pages_document_version_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX document_pages_document_version_idx ON public.document_pages USING btree (document_id, parser_version, page_number);


--
-- Name: document_references_doi_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX document_references_doi_idx ON public.document_references USING btree (lower(doi)) WHERE (doi IS NOT NULL);


--
-- Name: documents_object_key_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX documents_object_key_uq ON public.documents USING btree (object_key) WHERE (object_key IS NOT NULL);


--
-- Name: documents_project_report_status_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX documents_project_report_status_idx ON public.documents USING btree (project_id, report_id, status, updated_at DESC, id);


--
-- Name: documents_report_content_hash_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX documents_report_content_hash_uq ON public.documents USING btree (project_id, report_id, content_hash) WHERE (content_hash IS NOT NULL);


--
-- Name: documents_report_external_url_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX documents_report_external_url_uq ON public.documents USING btree (project_id, report_id, external_url) WHERE (external_url IS NOT NULL);


--
-- Name: documents_report_status_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX documents_report_status_idx ON public.documents USING btree (report_id, status, created_at DESC);


--
-- Name: durable_runs_active_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX durable_runs_active_idx ON public.durable_runs USING btree (status) WHERE (status = ANY (ARRAY['queued'::text, 'running'::text, 'waiting'::text]));


--
-- Name: durable_runs_project_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX durable_runs_project_created_idx ON public.durable_runs USING btree (project_id, created_at DESC, id DESC);


--
-- Name: durable_steps_run_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX durable_steps_run_idx ON public.durable_steps USING btree (run_id, ordinal, created_at);


--
-- Name: eligibility_criteria_protocol_id_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX eligibility_criteria_protocol_id_uq ON public.eligibility_criteria USING btree (protocol_version_id, id);


--
-- Name: eligibility_criteria_protocol_ordinal_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX eligibility_criteria_protocol_ordinal_idx ON public.eligibility_criteria USING btree (protocol_version_id, ordinal);


--
-- Name: exclusion_reasons_project_stage_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX exclusion_reasons_project_stage_idx ON public.exclusion_reasons USING btree (project_id, stage, code);


--
-- Name: extraction_events_project_study_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX extraction_events_project_study_idx ON public.extraction_events USING btree (project_id, study_id, created_at DESC, id DESC);


--
-- Name: extraction_field_definitions_project_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX extraction_field_definitions_project_idx ON public.extraction_field_definitions USING btree (project_id, field_key, version DESC);


--
-- Name: extraction_values_current_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX extraction_values_current_uq ON public.extraction_values USING btree (project_id, study_id, field_definition_id, field_definition_version) WHERE (superseded_at IS NULL);


--
-- Name: extraction_values_project_study_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX extraction_values_project_study_idx ON public.extraction_values USING btree (project_id, study_id, created_at DESC, id DESC);


--
-- Name: extraction_values_source_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX extraction_values_source_idx ON public.extraction_values USING btree (project_id, report_id, source_document_id, source_block_id);


--
-- Name: extraction_values_to_verify_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX extraction_values_to_verify_idx ON public.extraction_values USING btree (project_id, study_id) WHERE (needs_verification AND (superseded_at IS NULL));


--
-- Name: integration_inbox_claim_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX integration_inbox_claim_idx ON public.integration_inbox_events USING btree (next_attempt_at, received_at) WHERE (processing_state = ANY (ARRAY['received'::text, 'failed'::text, 'processing'::text]));


--
-- Name: integration_inbox_payload_expiry_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX integration_inbox_payload_expiry_idx ON public.integration_inbox_events USING btree (payload_expires_at) WHERE ((processing_state = 'processed'::text) AND (payload IS NOT NULL));


--
-- Name: integration_inbox_quarantine_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX integration_inbox_quarantine_idx ON public.integration_inbox_events USING btree (received_at) WHERE (processing_state = 'quarantined'::text);


--
-- Name: jobs_claim_capability_wait_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX jobs_claim_capability_wait_idx ON public.jobs USING btree (workload_class, priority DESC, available_at) WHERE (state = 'capability_wait'::text);


--
-- Name: jobs_claim_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX jobs_claim_idx ON public.jobs USING btree (state, priority DESC, available_at) WHERE (state = 'queued'::text);


--
-- Name: jobs_claim_pool_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX jobs_claim_pool_idx ON public.jobs USING btree (workload_class, priority DESC, available_at) WHERE (state = 'queued'::text);


--
-- Name: jobs_dedupe_kind_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX jobs_dedupe_kind_idx ON public.jobs USING btree (kind, dedupe_key);


--
-- Name: jobs_expired_running_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX jobs_expired_running_idx ON public.jobs USING btree (leased_until) WHERE (state = 'running'::text);


--
-- Name: jobs_unemitted_wakeup_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX jobs_unemitted_wakeup_idx ON public.jobs USING btree (available_at, id) WHERE ((state = ANY (ARRAY['queued'::text, 'capability_wait'::text])) AND (wakeup_emitted_generation < wakeup_generation));


--
-- Name: legacy_actor_mappings_user_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX legacy_actor_mappings_user_idx ON public.legacy_actor_mappings USING btree (user_id, legacy_actor_kind, legacy_actor_id);


--
-- Name: legacy_actor_non_identities_user_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX legacy_actor_non_identities_user_idx ON public.legacy_actor_non_identities USING btree (user_id, legacy_actor_kind, legacy_actor_id);


--
-- Name: metric_snapshots_latest_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX metric_snapshots_latest_idx ON public.metric_snapshots USING btree (project_id, metrics_as_of DESC);


--
-- Name: notifications_project_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX notifications_project_idx ON public.notifications USING btree (project_id, revision DESC);


--
-- Name: notifications_recent_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX notifications_recent_idx ON public.notifications USING btree (revision DESC);


--
-- Name: notifications_unread_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX notifications_unread_idx ON public.notifications USING btree (revision DESC) WHERE (read_at IS NULL);


--
-- Name: object_lifecycle_due_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX object_lifecycle_due_idx ON public.object_lifecycle USING btree (cleanup_after) WHERE ((deleted_at IS NULL) AND (cleanup_after IS NOT NULL));


--
-- Name: object_lifecycle_unfinished_retirement_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX object_lifecycle_unfinished_retirement_idx ON public.object_lifecycle USING btree (retired_at) WHERE ((retired_at IS NOT NULL) AND (deleted_at IS NULL));


--
-- Name: object_privacy_pending; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX object_privacy_pending ON public.object_lifecycle USING btree (privacy_due_at, storage_key) WHERE ((privacy_due_at IS NOT NULL) AND (deleted_at IS NULL));


--
-- Name: object_retained_references_expiry_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX object_retained_references_expiry_idx ON public.object_retained_references USING btree (expires_at);


--
-- Name: object_retained_references_storage_expiry_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX object_retained_references_storage_expiry_idx ON public.object_retained_references USING btree (storage_key, expires_at);


--
-- Name: operational_rate_limits_api_key_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX operational_rate_limits_api_key_idx ON public.operational_rate_limits USING btree (api_key_id) WHERE (api_key_id IS NOT NULL);


--
-- Name: operational_rate_limits_workflow_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX operational_rate_limits_workflow_idx ON public.operational_rate_limits USING btree (workflow_id) WHERE (workflow_id IS NOT NULL);


--
-- Name: outbox_events_claim_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX outbox_events_claim_idx ON public.outbox_events USING btree (next_attempt_at, created_at) WHERE (delivery_state = 'pending'::text);


--
-- Name: outbox_events_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX outbox_events_created_idx ON public.outbox_events USING btree (created_at);


--
-- Name: outbox_events_unpublished_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX outbox_events_unpublished_idx ON public.outbox_events USING btree (created_at) WHERE (published_at IS NULL);


--
-- Name: outbox_publication_due_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX outbox_publication_due_idx ON public.outbox_events USING btree (publication_next_at, created_at, event_id) WHERE ((published_at IS NULL) AND (delivery_state = 'pending'::text));


--
-- Name: outbox_receipts_pending_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX outbox_receipts_pending_idx ON public.outbox_receipts USING btree (event_id, next_attempt_at) WHERE (state = 'retrying'::text);


--
-- Name: pmid_import_items_queued_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX pmid_import_items_queued_idx ON public.pmid_import_items USING btree (acquisition_run_id, "position") WHERE (status = 'queued'::text);


--
-- Name: project_access_user_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX project_access_user_idx ON public.project_access USING btree (user_id);


--
-- Name: project_reports_project_report_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX project_reports_project_report_idx ON public.project_reports USING btree (project_id, report_id);


--
-- Name: project_reports_rank_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX project_reports_rank_idx ON public.project_reports USING btree (project_id, rank_score DESC, internal_citations DESC, report_id);


--
-- Name: project_reports_screening_queue_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX project_reports_screening_queue_idx ON public.project_reports USING btree (project_id, created_at, report_id);


--
-- Name: projection_state_identity_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX projection_state_identity_idx ON public.projection_state USING btree (projection_name, COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid));


--
-- Name: projects_workspace_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX projects_workspace_idx ON public.projects USING btree (workspace_id);


--
-- Name: protocol_versions_one_current_published_per_project_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX protocol_versions_one_current_published_per_project_idx ON public.protocol_versions USING btree (project_id) WHERE (status = 'published'::text);


--
-- Name: protocol_versions_one_draft_per_project_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX protocol_versions_one_draft_per_project_idx ON public.protocol_versions USING btree (project_id) WHERE (status = 'draft'::text);


--
-- Name: record_identifiers_lookup_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX record_identifiers_lookup_idx ON public.record_identifiers USING btree (scheme, normalized_value);


--
-- Name: records_project_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX records_project_created_idx ON public.records USING btree (project_id, created_at DESC);


--
-- Name: records_unresolved_project_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX records_unresolved_project_idx ON public.records USING btree (project_id, created_at, id) WHERE (report_id IS NULL);


--
-- Name: report_fetch_state_doi_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX report_fetch_state_doi_idx ON public.report_fetch_state USING btree (canonical_doi);


--
-- Name: report_fetch_state_stale_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX report_fetch_state_stale_idx ON public.report_fetch_state USING btree (lease_expires_at) WHERE (status = 'fetching'::text);


--
-- Name: reports_normalized_title_trgm_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reports_normalized_title_trgm_idx ON public.reports USING gin (normalized_title public.gin_trgm_ops);


--
-- Name: reports_title_abstract_trgm_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reports_title_abstract_trgm_idx ON public.reports USING gin (lower(((COALESCE(title, ''::text) || ' '::text) || COALESCE(abstract_text, ''::text))) public.gin_trgm_ops);


--
-- Name: reports_title_trgm_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reports_title_trgm_idx ON public.reports USING gin (lower(COALESCE(title, ''::text)) public.gin_trgm_ops);


--
-- Name: review_attempt_details_accepted_fingerprint_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX review_attempt_details_accepted_fingerprint_uq ON public.review_attempt_details USING btree (project_id, node_id, input_fingerprint) WHERE (accepted_at IS NOT NULL);


--
-- Name: review_calibration_bundles_exact_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX review_calibration_bundles_exact_idx ON public.review_calibration_bundles USING btree (project_id, definition_key, semantic_bundle_hash, status);


--
-- Name: review_calibration_bundles_stage_admission_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX review_calibration_bundles_stage_admission_idx ON public.review_calibration_bundles USING btree (project_id, definition_key, stage, status, evaluated_at DESC);


--
-- Name: review_run_manifests_generation_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX review_run_manifests_generation_idx ON public.review_run_manifests USING btree (project_id, manifest_hash, generation);


--
-- Name: review_run_manifests_project_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX review_run_manifests_project_created_idx ON public.review_run_manifests USING btree (project_id, created_at DESC, automation_run_id DESC);


--
-- Name: review_run_manifests_semantic_bundle_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX review_run_manifests_semantic_bundle_idx ON public.review_run_manifests USING btree (project_id, semantic_bundle_hash, definition_key);


--
-- Name: scientific_assignments_active_user_role; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX scientific_assignments_active_user_role ON public.scientific_assignments USING btree (user_id, role, project_id) WHERE (revoked_at IS NULL);


--
-- Name: scientific_assignments_one_active_role; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX scientific_assignments_one_active_role ON public.scientific_assignments USING btree (project_id, user_id, role) WHERE (revoked_at IS NULL);


--
-- Name: screening_events_assistant_plan_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX screening_events_assistant_plan_idx ON public.screening_events USING btree (project_id, report_id) WHERE (origin = 'assistant_plan'::text);


--
-- Name: screening_events_history_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX screening_events_history_idx ON public.screening_events USING btree (project_id, report_id, created_at DESC, id DESC);


--
-- Name: screening_events_report_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX screening_events_report_created_idx ON public.screening_events USING btree (project_id, report_id, created_at DESC);


--
-- Name: screening_state_title_status_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX screening_state_title_status_idx ON public.screening_state USING btree (project_id, title_abstract_status, report_id);


--
-- Name: screening_state_title_status_report_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX screening_state_title_status_report_idx ON public.screening_state USING btree (project_id, title_abstract_status, report_id);


--
-- Name: study_events_project_report_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX study_events_project_report_created_idx ON public.study_events USING btree (project_id, report_id, created_at, id);


--
-- Name: study_events_project_study_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX study_events_project_study_created_idx ON public.study_events USING btree (project_id, study_id, created_at, id);


--
-- Name: study_reports_project_report_uq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX study_reports_project_report_uq ON public.study_reports USING btree (project_id, report_id);


--
-- Name: study_reports_project_study_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX study_reports_project_study_idx ON public.study_reports USING btree (project_id, study_id, created_at, report_id);


--
-- Name: user_sessions_user_active_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX user_sessions_user_active_idx ON public.user_sessions USING btree (user_id, expires_at DESC) WHERE (revoked_at IS NULL);


--
-- Name: workflow_files_run_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX workflow_files_run_idx ON public.workflow_files USING btree (run_id);


--
-- Name: workflow_files_storage_key_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX workflow_files_storage_key_idx ON public.workflow_files USING btree (storage_key) WHERE (storage_key IS NOT NULL);


--
-- Name: workflow_files_unmigrated_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX workflow_files_unmigrated_idx ON public.workflow_files USING btree (project_id, id) WHERE (storage_key IS NULL);


--
-- Name: workflows_due_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX workflows_due_idx ON public.workflows USING btree (next_fire_at) WHERE ((status = 'enabled'::text) AND (next_fire_at IS NOT NULL));


--
-- Name: workflows_event_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX workflows_event_idx ON public.workflows USING btree (project_id, trigger_kind) WHERE (status = 'enabled'::text);


--
-- Name: workspace_invitations_email_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX workspace_invitations_email_idx ON public.workspace_invitations USING btree (email) WHERE (status = 'pending'::text);


--
-- Name: workspace_invitations_pending_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX workspace_invitations_pending_idx ON public.workspace_invitations USING btree (workspace_id, email) WHERE (status = 'pending'::text);


--
-- Name: workspace_memberships_user_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX workspace_memberships_user_idx ON public.workspace_memberships USING btree (user_id) WHERE (status = 'active'::text);


--
-- Name: jobs account_export_jobs_fence_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER account_export_jobs_fence_trigger BEFORE INSERT OR UPDATE ON public.jobs FOR EACH ROW EXECUTE FUNCTION public.account_export_jobs_fence();


--
-- Name: account_data_exports account_export_transition_guard_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER account_export_transition_guard_trigger BEFORE UPDATE ON public.account_data_exports FOR EACH ROW EXECUTE FUNCTION public.account_export_transition_guard();


--
-- Name: users account_inactivity_contact_changed; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER account_inactivity_contact_changed AFTER UPDATE OF verified_email ON public.users FOR EACH ROW WHEN ((old.verified_email IS DISTINCT FROM new.verified_email)) EXECUTE FUNCTION public.invalidate_inactivity_contact();


--
-- Name: admin_audit_events admin_audit_events_append_only_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER admin_audit_events_append_only_trigger BEFORE DELETE OR UPDATE ON public.admin_audit_events FOR EACH ROW EXECUTE FUNCTION public.reject_admin_audit_event_changes();


--
-- Name: admin_audit_events admin_audit_events_no_truncate_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER admin_audit_events_no_truncate_trigger BEFORE TRUNCATE ON public.admin_audit_events FOR EACH STATEMENT EXECUTE FUNCTION public.reject_admin_audit_event_changes();


--
-- Name: ai_screening_audit_labels ai_first_audit_label_identity_guard; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_first_audit_label_identity_guard BEFORE INSERT OR UPDATE OF actor_id, actor_user_id ON public.ai_screening_audit_labels FOR EACH ROW EXECUTE FUNCTION public.guard_ai_first_audit_label_identity();


--
-- Name: ai_screening_cohorts ai_first_design_frozen; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_first_design_frozen BEFORE UPDATE ON public.ai_screening_cohorts FOR EACH ROW EXECUTE FUNCTION public.protect_ai_first_evidence();


--
-- Name: ai_screening_dispositions ai_first_disposition_immutable; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_first_disposition_immutable BEFORE UPDATE ON public.ai_screening_dispositions FOR EACH ROW EXECUTE FUNCTION public.protect_ai_first_evidence();


--
-- Name: ai_screening_cohort_members ai_first_frame_frozen; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_first_frame_frozen BEFORE UPDATE ON public.ai_screening_cohort_members FOR EACH ROW EXECUTE FUNCTION public.protect_ai_first_frame();


--
-- Name: ai_screening_cohorts ai_first_identity_transition_guard; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_first_identity_transition_guard BEFORE UPDATE OF status ON public.ai_screening_cohorts FOR EACH ROW EXECUTE FUNCTION public.guard_ai_first_identity_transition();


--
-- Name: ai_screening_audit_labels ai_first_label_immutable; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_first_label_immutable BEFORE UPDATE ON public.ai_screening_audit_labels FOR EACH ROW EXECUTE FUNCTION public.protect_ai_first_evidence();


--
-- Name: ai_opinion_exposures ai_opinion_exposure_append_only_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_opinion_exposure_append_only_trigger BEFORE UPDATE ON public.ai_opinion_exposures FOR EACH ROW EXECUTE FUNCTION public.reject_ai_opinion_exposure_changes();


--
-- Name: ai_opinion_exposures ai_opinion_exposures_clear_stale_actor_provenance; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_opinion_exposures_clear_stale_actor_provenance BEFORE UPDATE OF audience_actor_id, audience_actor_user_id ON public.ai_opinion_exposures FOR EACH ROW EXECUTE FUNCTION public.clear_stale_actor_user_provenance('audience_actor_id', 'audience_actor_user_id');


--
-- Name: ai_proposals ai_proposal_availability; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_proposal_availability AFTER INSERT ON public.ai_proposals FOR EACH ROW EXECUTE FUNCTION public.track_ai_opinion_availability();


--
-- Name: ai_runs ai_run_parent_automation_project_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_run_parent_automation_project_trigger BEFORE INSERT OR UPDATE OF project_id, parent_automation_run_id ON public.ai_runs FOR EACH ROW EXECUTE FUNCTION public.enforce_ai_run_parent_automation_project();


--
-- Name: ai_screening_audit_labels ai_screening_audit_labels_clear_stale_actor_provenance; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_screening_audit_labels_clear_stale_actor_provenance BEFORE UPDATE OF actor_id, actor_user_id ON public.ai_screening_audit_labels FOR EACH ROW EXECUTE FUNCTION public.clear_stale_actor_user_provenance('actor_id', 'actor_user_id');


--
-- Name: ai_screening_cohorts ai_screening_cohorts_clear_stale_actor_provenance; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_screening_cohorts_clear_stale_actor_provenance BEFORE UPDATE OF approved_by, approved_by_actor_user_id, finalized_by, finalized_by_actor_user_id ON public.ai_screening_cohorts FOR EACH ROW EXECUTE FUNCTION public.clear_stale_actor_user_provenance('approved_by', 'approved_by_actor_user_id', 'finalized_by', 'finalized_by_actor_user_id');


--
-- Name: jobs ai_usage_jobs_fence_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_usage_jobs_fence_trigger BEFORE INSERT OR UPDATE ON public.jobs FOR EACH ROW EXECUTE FUNCTION public.ai_usage_jobs_fence();


--
-- Name: jobs ai_usage_jobs_write_lock_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_usage_jobs_write_lock_trigger BEFORE INSERT OR UPDATE ON public.jobs FOR EACH STATEMENT EXECUTE FUNCTION public.ai_usage_jobs_write_lock();


--
-- Name: ai_usage_ledger ai_usage_ledger_count_legacy_spend_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_usage_ledger_count_legacy_spend_trigger AFTER INSERT ON public.ai_usage_ledger FOR EACH ROW EXECUTE FUNCTION public.ai_usage_ledger_count_legacy_spend();


--
-- Name: ai_usage_ledger ai_usage_ledger_fill_legacy_identity_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_usage_ledger_fill_legacy_identity_trigger BEFORE INSERT ON public.ai_usage_ledger FOR EACH ROW EXECUTE FUNCTION public.ai_usage_ledger_fill_legacy_identity();


--
-- Name: ai_usage_corrections ai_usage_register_correction_fact_id_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_usage_register_correction_fact_id_trigger BEFORE INSERT OR UPDATE OF id ON public.ai_usage_corrections FOR EACH ROW EXECUTE FUNCTION public.ai_usage_register_fact_id('correction');


--
-- Name: ai_usage_ledger ai_usage_register_usage_fact_id_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_usage_register_usage_fact_id_trigger BEFORE INSERT OR UPDATE OF id ON public.ai_usage_ledger FOR EACH ROW EXECUTE FUNCTION public.ai_usage_register_fact_id('usage');


--
-- Name: projects ai_usage_sync_new_project_limit_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER ai_usage_sync_new_project_limit_trigger AFTER INSERT OR UPDATE OF ai_monthly_budget_micros, workspace_id ON public.projects FOR EACH ROW EXECUTE FUNCTION public.ai_usage_sync_new_project_limit();


--
-- Name: automation_run_details automation_run_details_identity; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER automation_run_details_identity BEFORE UPDATE ON public.automation_run_details FOR EACH ROW EXECUTE FUNCTION public.protect_durable_identity();


--
-- Name: automation_step_details automation_step_details_identity; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER automation_step_details_identity BEFORE UPDATE ON public.automation_step_details FOR EACH ROW EXECUTE FUNCTION public.protect_durable_identity();


--
-- Name: project_ai_autonomy autonomy_opinion_availability; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER autonomy_opinion_availability AFTER INSERT OR UPDATE ON public.project_ai_autonomy FOR EACH ROW EXECUTE FUNCTION public.track_ai_opinion_availability();


--
-- Name: billing_accounts billing_preserve_closure_context_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER billing_preserve_closure_context_trigger BEFORE INSERT OR UPDATE ON public.billing_accounts FOR EACH ROW EXECUTE FUNCTION public.billing_preserve_closure_context();


--
-- Name: workspaces billing_workspace_detach; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER billing_workspace_detach BEFORE DELETE ON public.workspaces FOR EACH ROW EXECUTE FUNCTION public.billing_detach_workspace();


--
-- Name: documents documents_object_lifecycle; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER documents_object_lifecycle AFTER INSERT OR DELETE OR UPDATE ON public.documents FOR EACH ROW EXECUTE FUNCTION public.object_reference_changed();


--
-- Name: durable_attempts durable_attempts_identity; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER durable_attempts_identity BEFORE UPDATE ON public.durable_attempts FOR EACH ROW EXECUTE FUNCTION public.protect_durable_identity();


--
-- Name: durable_runs durable_runs_identity; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER durable_runs_identity BEFORE UPDATE ON public.durable_runs FOR EACH ROW EXECUTE FUNCTION public.protect_durable_identity();


--
-- Name: durable_steps durable_steps_identity; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER durable_steps_identity BEFORE UPDATE ON public.durable_steps FOR EACH ROW EXECUTE FUNCTION public.protect_durable_identity();


--
-- Name: eligibility_criteria eligibility_criteria_immutable_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER eligibility_criteria_immutable_trigger BEFORE INSERT OR DELETE OR UPDATE ON public.eligibility_criteria FOR EACH ROW EXECUTE FUNCTION public.prevent_published_criterion_mutation();


--
-- Name: exclusion_reasons exclusion_reasons_touch_updated_at; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER exclusion_reasons_touch_updated_at BEFORE UPDATE ON public.exclusion_reasons FOR EACH ROW EXECUTE FUNCTION public.deepref_touch_exclusion_reason_updated_at();


--
-- Name: fulfilled_account_deletions fulfilled_account_physical_proof; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER fulfilled_account_physical_proof BEFORE INSERT OR UPDATE ON public.fulfilled_account_deletions FOR EACH ROW EXECUTE FUNCTION public.enforce_account_physical_proof();


--
-- Name: jobs jobs_claim_identity_increment; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER jobs_claim_identity_increment BEFORE UPDATE OF state ON public.jobs FOR EACH ROW WHEN (((old.state IS DISTINCT FROM 'running'::text) AND (new.state = 'running'::text))) EXECUTE FUNCTION public.jobs_increment_claim_seq();


--
-- Name: jobs jobs_settle_wakeup; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER jobs_settle_wakeup AFTER UPDATE OF state, available_at ON public.jobs FOR EACH ROW EXECUTE FUNCTION public.jobs_settle_wakeup();


--
-- Name: jobs jobs_workload_class_fill; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER jobs_workload_class_fill BEFORE INSERT OR UPDATE OF kind ON public.jobs FOR EACH ROW WHEN ((new.workload_class IS NULL)) EXECUTE FUNCTION public.jobs_fill_workload_class();


--
-- Name: jobs jobs_z_prepare_wakeup; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER jobs_z_prepare_wakeup BEFORE INSERT OR UPDATE OF state, available_at, wakeup_emitted_generation ON public.jobs FOR EACH ROW EXECUTE FUNCTION public.jobs_prepare_wakeup();


--
-- Name: jobs jobs_z_publish_wakeup; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER jobs_z_publish_wakeup AFTER INSERT OR UPDATE OF state, available_at, wakeup_emitted_generation ON public.jobs FOR EACH ROW EXECUTE FUNCTION public.jobs_publish_wakeup();


--
-- Name: jobs object_cleanup_jobs_fence_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER object_cleanup_jobs_fence_trigger BEFORE INSERT OR UPDATE ON public.jobs FOR EACH ROW EXECUTE FUNCTION public.object_cleanup_jobs_fence();


--
-- Name: object_retained_references object_retention_target_guard; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER object_retention_target_guard BEFORE INSERT OR UPDATE ON public.object_retained_references FOR EACH ROW EXECUTE FUNCTION public.enforce_object_retention_target();


--
-- Name: project_ai_screening_authority project_ai_screening_authority_clear_stale_actor_provenance; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER project_ai_screening_authority_clear_stale_actor_provenance BEFORE UPDATE OF approved_by, approved_by_actor_user_id ON public.project_ai_screening_authority FOR EACH ROW EXECUTE FUNCTION public.clear_stale_actor_user_provenance('approved_by', 'approved_by_actor_user_id');


--
-- Name: projects projects_seed_full_text_reasons; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER projects_seed_full_text_reasons AFTER INSERT ON public.projects FOR EACH ROW EXECUTE FUNCTION public.deepref_seed_full_text_reasons_on_project();


--
-- Name: projects projects_workspace_default_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER projects_workspace_default_trigger BEFORE INSERT ON public.projects FOR EACH ROW WHEN ((new.workspace_id IS NULL)) EXECUTE FUNCTION public.assign_project_workspace_default();


--
-- Name: protocol_versions protocol_versions_immutable_content_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER protocol_versions_immutable_content_trigger BEFORE DELETE OR UPDATE ON public.protocol_versions FOR EACH ROW EXECUTE FUNCTION public.prevent_published_protocol_content_mutation();


--
-- Name: review_artifacts review_artifact_immutable_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER review_artifact_immutable_trigger BEFORE UPDATE ON public.review_artifacts FOR EACH ROW EXECUTE FUNCTION public.reject_review_artifact_changes();


--
-- Name: review_attempt_details review_attempt_detail_immutable; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER review_attempt_detail_immutable BEFORE UPDATE ON public.review_attempt_details FOR EACH ROW EXECUTE FUNCTION public.protect_review_attempt_detail();


--
-- Name: review_calibration_bundles review_calibration_bundle_immutable_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER review_calibration_bundle_immutable_trigger BEFORE UPDATE ON public.review_calibration_bundles FOR EACH ROW EXECUTE FUNCTION public.reject_review_calibration_bundle_changes();


--
-- Name: review_calibration_bundles review_calibration_bundle_scheme_insert_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER review_calibration_bundle_scheme_insert_trigger BEFORE INSERT ON public.review_calibration_bundles FOR EACH ROW EXECUTE FUNCTION public.reject_legacy_calibration_identity();


--
-- Name: review_run_manifests review_run_manifest_immutable_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER review_run_manifest_immutable_trigger BEFORE UPDATE ON public.review_run_manifests FOR EACH ROW EXECUTE FUNCTION public.reject_immutable_review_manifest_changes();


--
-- Name: ai_reviewer_decisions reviewer_opinion_availability; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER reviewer_opinion_availability AFTER INSERT OR UPDATE ON public.ai_reviewer_decisions FOR EACH ROW EXECUTE FUNCTION public.track_ai_opinion_availability();


--
-- Name: screening_events screening_events_clear_stale_actor_provenance; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER screening_events_clear_stale_actor_provenance BEFORE UPDATE OF actor_id, actor_user_id ON public.screening_events FOR EACH ROW EXECUTE FUNCTION public.clear_stale_actor_user_provenance('actor_id', 'actor_user_id');


--
-- Name: screening_state screening_opinion_availability; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER screening_opinion_availability AFTER INSERT OR UPDATE ON public.screening_state FOR EACH ROW EXECUTE FUNCTION public.track_ai_opinion_availability();


--
-- Name: users users_anonymized_account_guard; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER users_anonymized_account_guard BEFORE UPDATE ON public.users FOR EACH ROW EXECUTE FUNCTION public.enforce_anonymized_deleted_user();


--
-- Name: workflow_files workflow_file_storage_guard_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER workflow_file_storage_guard_trigger BEFORE INSERT OR UPDATE ON public.workflow_files FOR EACH ROW EXECUTE FUNCTION public.workflow_file_storage_guard();


--
-- Name: workflow_files workflow_files_object_lifecycle; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER workflow_files_object_lifecycle AFTER INSERT OR DELETE OR UPDATE ON public.workflow_files FOR EACH ROW EXECUTE FUNCTION public.object_reference_changed();


--
-- Name: workflow_run_details workflow_run_details_identity; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER workflow_run_details_identity BEFORE UPDATE ON public.workflow_run_details FOR EACH ROW EXECUTE FUNCTION public.protect_durable_identity();


--
-- Name: workflow_step_details workflow_step_details_identity; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER workflow_step_details_identity BEFORE UPDATE ON public.workflow_step_details FOR EACH ROW EXECUTE FUNCTION public.protect_durable_identity();


--
-- Name: workflow_versions workflow_versions_immutable_trigger; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER workflow_versions_immutable_trigger BEFORE DELETE OR UPDATE ON public.workflow_versions FOR EACH ROW EXECUTE FUNCTION public.workflow_versions_immutable();


--
-- Name: account_data_exports account_data_exports_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_data_exports
    ADD CONSTRAINT account_data_exports_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: account_deletion_requests account_deletion_requests_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_deletion_requests
    ADD CONSTRAINT account_deletion_requests_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: account_erasure_objects account_erasure_objects_storage_key_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_erasure_objects
    ADD CONSTRAINT account_erasure_objects_storage_key_fkey FOREIGN KEY (storage_key) REFERENCES public.object_lifecycle(storage_key);


--
-- Name: account_erasure_objects account_erasure_objects_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_erasure_objects
    ADD CONSTRAINT account_erasure_objects_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.account_erasure_progress(user_id) ON DELETE CASCADE;


--
-- Name: account_erasure_progress account_erasure_progress_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_erasure_progress
    ADD CONSTRAINT account_erasure_progress_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: account_erasure_writers account_erasure_writers_attempt_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_erasure_writers
    ADD CONSTRAINT account_erasure_writers_attempt_id_fkey FOREIGN KEY (attempt_id) REFERENCES public.account_export_writers(attempt_id);


--
-- Name: account_erasure_writers account_erasure_writers_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_erasure_writers
    ADD CONSTRAINT account_erasure_writers_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.account_erasure_progress(user_id) ON DELETE CASCADE;


--
-- Name: account_export_writers account_export_writers_export_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_export_writers
    ADD CONSTRAINT account_export_writers_export_id_fkey FOREIGN KEY (export_id) REFERENCES public.account_data_exports(id);


--
-- Name: account_human_activity account_human_activity_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.account_human_activity
    ADD CONSTRAINT account_human_activity_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: acquisition_items acquisition_items_acquisition_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.acquisition_items
    ADD CONSTRAINT acquisition_items_acquisition_id_fkey FOREIGN KEY (acquisition_id) REFERENCES public.acquisition_runs(id) ON DELETE CASCADE;


--
-- Name: acquisition_items acquisition_items_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.acquisition_items
    ADD CONSTRAINT acquisition_items_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: acquisition_items acquisition_items_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.acquisition_items
    ADD CONSTRAINT acquisition_items_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id);


--
-- Name: acquisition_runs acquisition_runs_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.acquisition_runs
    ADD CONSTRAINT acquisition_runs_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: admin_audit_events admin_audit_events_principal_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.admin_audit_events
    ADD CONSTRAINT admin_audit_events_principal_user_id_fkey FOREIGN KEY (principal_user_id) REFERENCES public.users(id);


--
-- Name: admin_audit_events admin_audit_events_target_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.admin_audit_events
    ADD CONSTRAINT admin_audit_events_target_user_id_fkey FOREIGN KEY (target_user_id) REFERENCES public.users(id);


--
-- Name: ai_activity ai_activity_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_activity
    ADD CONSTRAINT ai_activity_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: ai_autonomy_outbox ai_autonomy_outbox_project_id_proposal_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_autonomy_outbox
    ADD CONSTRAINT ai_autonomy_outbox_project_id_proposal_id_fkey FOREIGN KEY (project_id, proposal_id) REFERENCES public.ai_proposals(project_id, id) ON DELETE CASCADE;


--
-- Name: ai_opinion_exposures ai_opinion_exposures_ai_reviewer_decision_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_opinion_exposures
    ADD CONSTRAINT ai_opinion_exposures_ai_reviewer_decision_id_fkey FOREIGN KEY (ai_reviewer_decision_id) REFERENCES public.ai_reviewer_decisions(id) ON DELETE CASCADE;


--
-- Name: ai_opinion_exposures ai_opinion_exposures_audience_actor_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_opinion_exposures
    ADD CONSTRAINT ai_opinion_exposures_audience_actor_user_id_fkey FOREIGN KEY (audience_actor_user_id) REFERENCES public.users(id);


--
-- Name: ai_opinion_exposures ai_opinion_exposures_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_opinion_exposures
    ADD CONSTRAINT ai_opinion_exposures_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: ai_opinion_exposures ai_opinion_exposures_project_id_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_opinion_exposures
    ADD CONSTRAINT ai_opinion_exposures_project_id_report_id_fkey FOREIGN KEY (project_id, report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: ai_proposal_criterion_judgments ai_proposal_criterion_judgments_criterion_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_criterion_judgments
    ADD CONSTRAINT ai_proposal_criterion_judgments_criterion_fkey FOREIGN KEY (protocol_version_id, criterion_id) REFERENCES public.eligibility_criteria(protocol_version_id, id) ON DELETE RESTRICT;


--
-- Name: ai_proposal_criterion_judgments ai_proposal_criterion_judgments_proposal_project_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_criterion_judgments
    ADD CONSTRAINT ai_proposal_criterion_judgments_proposal_project_fkey FOREIGN KEY (proposal_id, project_id) REFERENCES public.ai_proposals(id, project_id) ON DELETE CASCADE;


--
-- Name: ai_proposal_criterion_judgments ai_proposal_criterion_judgments_proposal_protocol_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_criterion_judgments
    ADD CONSTRAINT ai_proposal_criterion_judgments_proposal_protocol_fkey FOREIGN KEY (proposal_id, project_id, protocol_version_id) REFERENCES public.ai_proposals(id, project_id, protocol_version_id) ON DELETE CASCADE;


--
-- Name: ai_proposal_criterion_judgments ai_proposal_criterion_judgments_protocol_version_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_criterion_judgments
    ADD CONSTRAINT ai_proposal_criterion_judgments_protocol_version_id_fkey FOREIGN KEY (project_id, protocol_version_id) REFERENCES public.protocol_versions(project_id, id) ON DELETE CASCADE;


--
-- Name: ai_proposal_evidence ai_proposal_evidence_document_block_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_evidence
    ADD CONSTRAINT ai_proposal_evidence_document_block_fkey FOREIGN KEY (document_id, document_block_id) REFERENCES public.document_blocks(document_id, id) ON DELETE RESTRICT;


--
-- Name: ai_proposal_evidence ai_proposal_evidence_document_project_report_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_evidence
    ADD CONSTRAINT ai_proposal_evidence_document_project_report_fkey FOREIGN KEY (project_id, report_id, document_id) REFERENCES public.documents(project_id, report_id, id) ON DELETE CASCADE;


--
-- Name: ai_proposal_evidence ai_proposal_evidence_project_report_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_evidence
    ADD CONSTRAINT ai_proposal_evidence_project_report_fkey FOREIGN KEY (project_id, report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: ai_proposal_evidence ai_proposal_evidence_proposal_project_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposal_evidence
    ADD CONSTRAINT ai_proposal_evidence_proposal_project_fkey FOREIGN KEY (proposal_id, project_id) REFERENCES public.ai_proposals(id, project_id) ON DELETE CASCADE;


--
-- Name: ai_proposals ai_proposals_ai_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposals
    ADD CONSTRAINT ai_proposals_ai_run_id_fkey FOREIGN KEY (ai_run_id) REFERENCES public.ai_runs(id) ON DELETE CASCADE;


--
-- Name: ai_proposals ai_proposals_model_run_project_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposals
    ADD CONSTRAINT ai_proposals_model_run_project_fk FOREIGN KEY (model_run_id, project_id) REFERENCES public.ai_runs(id, project_id) ON DELETE CASCADE;


--
-- Name: ai_proposals ai_proposals_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposals
    ADD CONSTRAINT ai_proposals_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: ai_proposals ai_proposals_project_protocol_target_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposals
    ADD CONSTRAINT ai_proposals_project_protocol_target_fkey FOREIGN KEY (project_id, protocol_version_id) REFERENCES public.protocol_versions(project_id, id) ON DELETE CASCADE;


--
-- Name: ai_proposals ai_proposals_project_record_target_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposals
    ADD CONSTRAINT ai_proposals_project_record_target_fkey FOREIGN KEY (project_id, target_record_id) REFERENCES public.records(project_id, id) ON DELETE CASCADE;


--
-- Name: ai_proposals ai_proposals_project_report_target_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposals
    ADD CONSTRAINT ai_proposals_project_report_target_fkey FOREIGN KEY (project_id, target_report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: ai_proposals ai_proposals_project_study_target_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_proposals
    ADD CONSTRAINT ai_proposals_project_study_target_fkey FOREIGN KEY (project_id, target_study_id) REFERENCES public.studies(project_id, id) ON DELETE CASCADE;


--
-- Name: ai_reviewer_decisions ai_reviewer_decisions_activity_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_reviewer_decisions
    ADD CONSTRAINT ai_reviewer_decisions_activity_id_fkey FOREIGN KEY (activity_id) REFERENCES public.ai_activity(id) ON DELETE SET NULL;


--
-- Name: ai_reviewer_decisions ai_reviewer_decisions_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_reviewer_decisions
    ADD CONSTRAINT ai_reviewer_decisions_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: ai_reviewer_decisions ai_reviewer_decisions_project_id_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_reviewer_decisions
    ADD CONSTRAINT ai_reviewer_decisions_project_id_report_id_fkey FOREIGN KEY (project_id, report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: ai_reviewer_decisions ai_reviewer_decisions_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_reviewer_decisions
    ADD CONSTRAINT ai_reviewer_decisions_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: ai_run_evidence ai_run_evidence_ai_run_id_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_run_evidence
    ADD CONSTRAINT ai_run_evidence_ai_run_id_project_id_fkey FOREIGN KEY (ai_run_id, project_id) REFERENCES public.ai_runs(id, project_id) ON DELETE CASCADE;


--
-- Name: ai_run_evidence ai_run_evidence_document_id_document_block_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_run_evidence
    ADD CONSTRAINT ai_run_evidence_document_id_document_block_id_fkey FOREIGN KEY (document_id, document_block_id) REFERENCES public.document_blocks(document_id, id) ON DELETE RESTRICT;


--
-- Name: ai_run_evidence ai_run_evidence_project_id_document_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_run_evidence
    ADD CONSTRAINT ai_run_evidence_project_id_document_id_fkey FOREIGN KEY (project_id, document_id) REFERENCES public.documents(project_id, id) ON DELETE CASCADE;


--
-- Name: ai_runs ai_runs_parent_automation_run_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_runs
    ADD CONSTRAINT ai_runs_parent_automation_run_fk FOREIGN KEY (parent_automation_run_id) REFERENCES public.durable_runs(id) ON DELETE SET NULL;


--
-- Name: ai_runs ai_runs_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_runs
    ADD CONSTRAINT ai_runs_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: ai_screening_audit_labels ai_screening_audit_labels_actor_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_audit_labels
    ADD CONSTRAINT ai_screening_audit_labels_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES public.users(id);


--
-- Name: ai_screening_audit_labels ai_screening_audit_labels_cohort_id_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_audit_labels
    ADD CONSTRAINT ai_screening_audit_labels_cohort_id_report_id_fkey FOREIGN KEY (cohort_id, report_id) REFERENCES public.ai_screening_cohort_members(cohort_id, report_id) ON DELETE CASCADE;


--
-- Name: ai_screening_audit_labels ai_screening_audit_labels_project_id_cohort_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_audit_labels
    ADD CONSTRAINT ai_screening_audit_labels_project_id_cohort_id_fkey FOREIGN KEY (project_id, cohort_id) REFERENCES public.ai_screening_cohorts(project_id, id) ON DELETE CASCADE;


--
-- Name: ai_screening_cohort_members ai_screening_cohort_members_project_id_cohort_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohort_members
    ADD CONSTRAINT ai_screening_cohort_members_project_id_cohort_id_fkey FOREIGN KEY (project_id, cohort_id) REFERENCES public.ai_screening_cohorts(project_id, id) ON DELETE CASCADE;


--
-- Name: ai_screening_cohort_members ai_screening_cohort_members_project_id_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohort_members
    ADD CONSTRAINT ai_screening_cohort_members_project_id_report_id_fkey FOREIGN KEY (project_id, report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: ai_screening_cohort_members ai_screening_cohort_members_project_id_review_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohort_members
    ADD CONSTRAINT ai_screening_cohort_members_project_id_review_run_id_fkey FOREIGN KEY (project_id, review_run_id) REFERENCES public.review_run_manifests(project_id, automation_run_id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ai_screening_cohorts ai_screening_cohorts_approved_by_actor_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohorts
    ADD CONSTRAINT ai_screening_cohorts_approved_by_actor_user_id_fkey FOREIGN KEY (approved_by_actor_user_id) REFERENCES public.users(id);


--
-- Name: ai_screening_cohorts ai_screening_cohorts_finalized_by_actor_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohorts
    ADD CONSTRAINT ai_screening_cohorts_finalized_by_actor_user_id_fkey FOREIGN KEY (finalized_by_actor_user_id) REFERENCES public.users(id);


--
-- Name: ai_screening_cohorts ai_screening_cohorts_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohorts
    ADD CONSTRAINT ai_screening_cohorts_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: ai_screening_cohorts ai_screening_cohorts_protocol_version_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_cohorts
    ADD CONSTRAINT ai_screening_cohorts_protocol_version_id_fkey FOREIGN KEY (protocol_version_id) REFERENCES public.protocol_versions(id) ON DELETE CASCADE;


--
-- Name: ai_screening_dispositions ai_screening_dispositions_ai_run_id_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_dispositions
    ADD CONSTRAINT ai_screening_dispositions_ai_run_id_project_id_fkey FOREIGN KEY (ai_run_id, project_id) REFERENCES public.ai_runs(id, project_id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ai_screening_dispositions ai_screening_dispositions_cohort_id_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_dispositions
    ADD CONSTRAINT ai_screening_dispositions_cohort_id_report_id_fkey FOREIGN KEY (cohort_id, report_id) REFERENCES public.ai_screening_cohort_members(cohort_id, report_id) ON DELETE CASCADE;


--
-- Name: ai_screening_dispositions ai_screening_dispositions_finalized_event_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_dispositions
    ADD CONSTRAINT ai_screening_dispositions_finalized_event_id_fkey FOREIGN KEY (finalized_event_id) REFERENCES public.screening_events(id) ON DELETE CASCADE;


--
-- Name: ai_screening_dispositions ai_screening_dispositions_project_id_cohort_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_dispositions
    ADD CONSTRAINT ai_screening_dispositions_project_id_cohort_id_fkey FOREIGN KEY (project_id, cohort_id) REFERENCES public.ai_screening_cohorts(project_id, id) ON DELETE CASCADE;


--
-- Name: ai_screening_dispositions ai_screening_dispositions_project_id_review_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_dispositions
    ADD CONSTRAINT ai_screening_dispositions_project_id_review_run_id_fkey FOREIGN KEY (project_id, review_run_id) REFERENCES public.review_run_manifests(project_id, automation_run_id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ai_screening_dispositions ai_screening_dispositions_protocol_version_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_screening_dispositions
    ADD CONSTRAINT ai_screening_dispositions_protocol_version_id_fkey FOREIGN KEY (protocol_version_id) REFERENCES public.protocol_versions(id) ON DELETE CASCADE;


--
-- Name: api_keys api_keys_created_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.users(id);


--
-- Name: api_keys api_keys_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE CASCADE;


--
-- Name: appraisal_assessment_evidence appraisal_assessment_evidence_assessment_id_project_id_rep_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_assessment_evidence
    ADD CONSTRAINT appraisal_assessment_evidence_assessment_id_project_id_rep_fkey FOREIGN KEY (assessment_id, project_id, report_id) REFERENCES public.appraisal_assessments(id, project_id, report_id) ON DELETE CASCADE;


--
-- Name: appraisal_assessment_evidence appraisal_assessment_evidence_document_id_block_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_assessment_evidence
    ADD CONSTRAINT appraisal_assessment_evidence_document_id_block_id_fkey FOREIGN KEY (document_id, block_id) REFERENCES public.document_blocks(document_id, id) ON DELETE CASCADE;


--
-- Name: appraisal_assessment_evidence appraisal_assessment_evidence_project_id_report_id_documen_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_assessment_evidence
    ADD CONSTRAINT appraisal_assessment_evidence_project_id_report_id_documen_fkey FOREIGN KEY (project_id, report_id, document_id) REFERENCES public.documents(project_id, report_id, id) ON DELETE CASCADE;


--
-- Name: appraisal_assessments appraisal_assessments_project_id_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_assessments
    ADD CONSTRAINT appraisal_assessments_project_id_report_id_fkey FOREIGN KEY (project_id, report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: appraisal_events appraisal_events_assessment_id_project_id_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_events
    ADD CONSTRAINT appraisal_events_assessment_id_project_id_report_id_fkey FOREIGN KEY (assessment_id, project_id, report_id) REFERENCES public.appraisal_assessments(id, project_id, report_id) ON DELETE CASCADE;


--
-- Name: appraisal_events appraisal_events_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_events
    ADD CONSTRAINT appraisal_events_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: appraisal_events appraisal_events_project_id_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.appraisal_events
    ADD CONSTRAINT appraisal_events_project_id_report_id_fkey FOREIGN KEY (project_id, report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: assistant_agent_runs assistant_agent_runs_conversation_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_agent_runs
    ADD CONSTRAINT assistant_agent_runs_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.assistant_conversations(id) ON DELETE CASCADE;


--
-- Name: assistant_agent_runs assistant_agent_runs_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_agent_runs
    ADD CONSTRAINT assistant_agent_runs_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: assistant_agent_runs assistant_agent_runs_trigger_message_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_agent_runs
    ADD CONSTRAINT assistant_agent_runs_trigger_message_id_fkey FOREIGN KEY (trigger_message_id) REFERENCES public.assistant_messages(id) ON DELETE CASCADE;


--
-- Name: assistant_conversations assistant_conversations_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_conversations
    ADD CONSTRAINT assistant_conversations_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: assistant_messages assistant_messages_conversation_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_messages
    ADD CONSTRAINT assistant_messages_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.assistant_conversations(id) ON DELETE CASCADE;


--
-- Name: assistant_plans assistant_plans_conversation_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_plans
    ADD CONSTRAINT assistant_plans_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.assistant_conversations(id) ON DELETE CASCADE;


--
-- Name: assistant_plans assistant_plans_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_plans
    ADD CONSTRAINT assistant_plans_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: assistant_run_events assistant_run_events_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_run_events
    ADD CONSTRAINT assistant_run_events_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.assistant_agent_runs(id) ON DELETE CASCADE;


--
-- Name: automation_definition_steps automation_definition_steps_project_id_definition_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_definition_steps
    ADD CONSTRAINT automation_definition_steps_project_id_definition_id_fkey FOREIGN KEY (project_id, definition_id) REFERENCES public.automation_definitions(project_id, id) ON DELETE CASCADE;


--
-- Name: automation_definitions automation_definitions_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_definitions
    ADD CONSTRAINT automation_definitions_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: automation_run_details automation_run_details_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_run_details
    ADD CONSTRAINT automation_run_details_id_fkey FOREIGN KEY (id) REFERENCES public.durable_runs(id) ON DELETE CASCADE;


--
-- Name: automation_run_details automation_run_details_project_id_definition_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_run_details
    ADD CONSTRAINT automation_run_details_project_id_definition_id_fkey FOREIGN KEY (project_id, definition_id) REFERENCES public.automation_definitions(project_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: automation_run_details automation_run_details_project_id_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_run_details
    ADD CONSTRAINT automation_run_details_project_id_id_fkey FOREIGN KEY (project_id, id) REFERENCES public.durable_runs(project_id, id) ON DELETE CASCADE;


--
-- Name: automation_step_details automation_step_details_accepted_attempt_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_step_details
    ADD CONSTRAINT automation_step_details_accepted_attempt_id_fkey FOREIGN KEY (accepted_attempt_id) REFERENCES public.durable_attempts(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: automation_step_details automation_step_details_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_step_details
    ADD CONSTRAINT automation_step_details_id_fkey FOREIGN KEY (id) REFERENCES public.durable_steps(id) ON DELETE CASCADE;


--
-- Name: automation_step_details automation_step_details_project_id_accepted_attempt_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_step_details
    ADD CONSTRAINT automation_step_details_project_id_accepted_attempt_id_fkey FOREIGN KEY (project_id, accepted_attempt_id) REFERENCES public.durable_attempts(project_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: automation_step_details automation_step_details_project_id_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_step_details
    ADD CONSTRAINT automation_step_details_project_id_run_id_fkey FOREIGN KEY (project_id, run_id) REFERENCES public.durable_runs(project_id, id) ON DELETE CASCADE;


--
-- Name: automation_step_details automation_step_details_project_id_run_id_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_step_details
    ADD CONSTRAINT automation_step_details_project_id_run_id_id_fkey FOREIGN KEY (project_id, run_id, id) REFERENCES public.durable_steps(project_id, run_id, id) ON DELETE CASCADE;


--
-- Name: billing_accounts billing_accounts_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_accounts
    ADD CONSTRAINT billing_accounts_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE SET NULL;


--
-- Name: billing_free_enrollments billing_free_enrollments_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_free_enrollments
    ADD CONSTRAINT billing_free_enrollments_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE CASCADE;


--
-- Name: billing_inbox_invalidations billing_inbox_invalidations_account_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_inbox_invalidations
    ADD CONSTRAINT billing_inbox_invalidations_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.billing_accounts(id) ON DELETE CASCADE;


--
-- Name: billing_inbox_invalidations billing_inbox_invalidations_inbox_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_inbox_invalidations
    ADD CONSTRAINT billing_inbox_invalidations_inbox_id_fkey FOREIGN KEY (inbox_id) REFERENCES public.integration_inbox_events(id);


--
-- Name: billing_usage_fact_targets billing_usage_fact_targets_account_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_usage_fact_targets
    ADD CONSTRAINT billing_usage_fact_targets_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.billing_accounts(id);


--
-- Name: billing_usage_fact_targets billing_usage_fact_targets_fact_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_usage_fact_targets
    ADD CONSTRAINT billing_usage_fact_targets_fact_id_fkey FOREIGN KEY (fact_id) REFERENCES public.outbox_events(event_id);


--
-- Name: billing_usage_resolutions billing_usage_resolutions_fact_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_usage_resolutions
    ADD CONSTRAINT billing_usage_resolutions_fact_id_fkey FOREIGN KEY (fact_id) REFERENCES public.outbox_events(event_id);


--
-- Name: billing_usage_resolutions billing_usage_resolutions_resolved_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.billing_usage_resolutions
    ADD CONSTRAINT billing_usage_resolutions_resolved_by_fkey FOREIGN KEY (resolved_by) REFERENCES public.users(id);


--
-- Name: citations citations_acquisition_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.citations
    ADD CONSTRAINT citations_acquisition_run_id_fkey FOREIGN KEY (acquisition_run_id) REFERENCES public.acquisition_runs(id) ON DELETE SET NULL;


--
-- Name: citations citations_project_id_fkey1; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.citations
    ADD CONSTRAINT citations_project_id_fkey1 FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: citations citations_source_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.citations
    ADD CONSTRAINT citations_source_report_id_fkey FOREIGN KEY (source_report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: citations citations_target_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.citations
    ADD CONSTRAINT citations_target_report_id_fkey FOREIGN KEY (target_report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: crossref_raw_records crossref_raw_records_canonical_doi_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.crossref_raw_records
    ADD CONSTRAINT crossref_raw_records_canonical_doi_fkey FOREIGN KEY (canonical_doi) REFERENCES public.works(canonical_doi) ON DELETE CASCADE;


--
-- Name: dedupe_proposals dedupe_proposals_candidate_project_report_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_proposals
    ADD CONSTRAINT dedupe_proposals_candidate_project_report_fkey FOREIGN KEY (project_id, candidate_report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: dedupe_proposals dedupe_proposals_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_proposals
    ADD CONSTRAINT dedupe_proposals_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: dedupe_proposals dedupe_proposals_project_id_record_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_proposals
    ADD CONSTRAINT dedupe_proposals_project_id_record_id_fkey FOREIGN KEY (project_id, record_id) REFERENCES public.records(project_id, id) ON DELETE CASCADE;


--
-- Name: dedupe_resolution_events dedupe_resolution_events_prior_project_report_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_resolution_events
    ADD CONSTRAINT dedupe_resolution_events_prior_project_report_fkey FOREIGN KEY (project_id, prior_report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: dedupe_resolution_events dedupe_resolution_events_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_resolution_events
    ADD CONSTRAINT dedupe_resolution_events_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: dedupe_resolution_events dedupe_resolution_events_project_id_record_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_resolution_events
    ADD CONSTRAINT dedupe_resolution_events_project_id_record_id_fkey FOREIGN KEY (project_id, record_id) REFERENCES public.records(project_id, id) ON DELETE CASCADE;


--
-- Name: dedupe_resolution_events dedupe_resolution_events_proposal_project_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_resolution_events
    ADD CONSTRAINT dedupe_resolution_events_proposal_project_fkey FOREIGN KEY (project_id, proposal_id) REFERENCES public.dedupe_proposals(project_id, id) ON DELETE CASCADE;


--
-- Name: dedupe_resolution_events dedupe_resolution_events_resolved_project_report_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_resolution_events
    ADD CONSTRAINT dedupe_resolution_events_resolved_project_report_fkey FOREIGN KEY (project_id, resolved_report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: dedupe_resolution_events dedupe_resolution_events_reverted_event_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dedupe_resolution_events
    ADD CONSTRAINT dedupe_resolution_events_reverted_event_fkey FOREIGN KEY (project_id, record_id, reverted_event_id) REFERENCES public.dedupe_resolution_events(project_id, record_id, id) ON DELETE CASCADE;


--
-- Name: document_block_embeddings document_block_embeddings_document_block_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_block_embeddings
    ADD CONSTRAINT document_block_embeddings_document_block_id_fkey FOREIGN KEY (document_block_id) REFERENCES public.document_blocks(id) ON DELETE CASCADE;


--
-- Name: document_blocks document_blocks_document_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_blocks
    ADD CONSTRAINT document_blocks_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.documents(id) ON DELETE CASCADE;


--
-- Name: document_blocks document_blocks_page_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_blocks
    ADD CONSTRAINT document_blocks_page_fk FOREIGN KEY (document_id, parser_version, page_number) REFERENCES public.document_pages(document_id, parser_version, page_number) ON DELETE CASCADE;


--
-- Name: document_pages document_pages_document_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_pages
    ADD CONSTRAINT document_pages_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.documents(id) ON DELETE CASCADE;


--
-- Name: document_references document_references_document_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_references
    ADD CONSTRAINT document_references_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.documents(id) ON DELETE CASCADE;


--
-- Name: document_sections document_sections_document_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.document_sections
    ADD CONSTRAINT document_sections_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.documents(id) ON DELETE CASCADE;


--
-- Name: documents documents_project_report_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.documents
    ADD CONSTRAINT documents_project_report_fk FOREIGN KEY (project_id, report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: documents documents_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.documents
    ADD CONSTRAINT documents_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: durable_attempts durable_attempts_project_id_job_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_attempts
    ADD CONSTRAINT durable_attempts_project_id_job_id_fkey FOREIGN KEY (project_id, job_id) REFERENCES public.jobs(project_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: durable_attempts durable_attempts_project_id_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_attempts
    ADD CONSTRAINT durable_attempts_project_id_run_id_fkey FOREIGN KEY (project_id, run_id) REFERENCES public.durable_runs(project_id, id) ON DELETE CASCADE;


--
-- Name: durable_attempts durable_attempts_project_id_step_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_attempts
    ADD CONSTRAINT durable_attempts_project_id_step_id_fkey FOREIGN KEY (project_id, step_id) REFERENCES public.durable_steps(project_id, id) ON DELETE CASCADE;


--
-- Name: durable_runs durable_runs_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_runs
    ADD CONSTRAINT durable_runs_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: durable_runs durable_runs_project_id_job_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_runs
    ADD CONSTRAINT durable_runs_project_id_job_id_fkey FOREIGN KEY (project_id, job_id) REFERENCES public.jobs(project_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: durable_steps durable_steps_project_id_claimed_job_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_steps
    ADD CONSTRAINT durable_steps_project_id_claimed_job_id_fkey FOREIGN KEY (project_id, claimed_job_id) REFERENCES public.jobs(project_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: durable_steps durable_steps_project_id_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.durable_steps
    ADD CONSTRAINT durable_steps_project_id_run_id_fkey FOREIGN KEY (project_id, run_id) REFERENCES public.durable_runs(project_id, id) ON DELETE CASCADE;


--
-- Name: eligibility_criteria eligibility_criteria_protocol_version_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.eligibility_criteria
    ADD CONSTRAINT eligibility_criteria_protocol_version_id_fkey FOREIGN KEY (protocol_version_id) REFERENCES public.protocol_versions(id) ON DELETE CASCADE;


--
-- Name: exclusion_reasons exclusion_reasons_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.exclusion_reasons
    ADD CONSTRAINT exclusion_reasons_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: extraction_events extraction_events_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_events
    ADD CONSTRAINT extraction_events_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: extraction_events extraction_events_project_id_proposal_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_events
    ADD CONSTRAINT extraction_events_project_id_proposal_id_fkey FOREIGN KEY (project_id, proposal_id) REFERENCES public.ai_proposals(project_id, id) ON DELETE CASCADE;


--
-- Name: extraction_events extraction_events_project_id_study_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_events
    ADD CONSTRAINT extraction_events_project_id_study_id_fkey FOREIGN KEY (project_id, study_id) REFERENCES public.studies(project_id, id) ON DELETE CASCADE;


--
-- Name: extraction_field_definitions extraction_field_definitions_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_field_definitions
    ADD CONSTRAINT extraction_field_definitions_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: extraction_values extraction_values_project_id_field_definition_id_field_def_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_values
    ADD CONSTRAINT extraction_values_project_id_field_definition_id_field_def_fkey FOREIGN KEY (project_id, field_definition_id, field_definition_version) REFERENCES public.extraction_field_definitions(project_id, id, version) ON DELETE RESTRICT;


--
-- Name: extraction_values extraction_values_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_values
    ADD CONSTRAINT extraction_values_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: extraction_values extraction_values_project_id_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_values
    ADD CONSTRAINT extraction_values_project_id_report_id_fkey FOREIGN KEY (project_id, report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: extraction_values extraction_values_project_id_report_id_source_document_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_values
    ADD CONSTRAINT extraction_values_project_id_report_id_source_document_id_fkey FOREIGN KEY (project_id, report_id, source_document_id) REFERENCES public.documents(project_id, report_id, id) ON DELETE CASCADE;


--
-- Name: extraction_values extraction_values_project_id_study_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_values
    ADD CONSTRAINT extraction_values_project_id_study_id_fkey FOREIGN KEY (project_id, study_id) REFERENCES public.studies(project_id, id) ON DELETE CASCADE;


--
-- Name: extraction_values extraction_values_source_document_id_source_block_id_sourc_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.extraction_values
    ADD CONSTRAINT extraction_values_source_document_id_source_block_id_sourc_fkey FOREIGN KEY (source_document_id, source_block_id, source_parser_version) REFERENCES public.document_blocks(document_id, id, parser_version) ON DELETE RESTRICT;


--
-- Name: fetched_citation_facts fetched_citation_facts_source_doi_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fetched_citation_facts
    ADD CONSTRAINT fetched_citation_facts_source_doi_fkey FOREIGN KEY (source_doi) REFERENCES public.works(canonical_doi) ON DELETE CASCADE;


--
-- Name: fetched_citation_facts fetched_citation_facts_target_doi_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fetched_citation_facts
    ADD CONSTRAINT fetched_citation_facts_target_doi_fkey FOREIGN KEY (target_doi) REFERENCES public.works(canonical_doi) ON DELETE CASCADE;


--
-- Name: fetched_unresolved_reference_facts fetched_unresolved_reference_facts_source_doi_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fetched_unresolved_reference_facts
    ADD CONSTRAINT fetched_unresolved_reference_facts_source_doi_fkey FOREIGN KEY (source_doi) REFERENCES public.works(canonical_doi) ON DELETE CASCADE;


--
-- Name: fulfilled_account_deletions fulfilled_account_deletions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.fulfilled_account_deletions
    ADD CONSTRAINT fulfilled_account_deletions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: jobs jobs_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.jobs
    ADD CONSTRAINT jobs_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: legacy_actor_mappings legacy_actor_mappings_confirmed_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legacy_actor_mappings
    ADD CONSTRAINT legacy_actor_mappings_confirmed_by_fkey FOREIGN KEY (confirmed_by) REFERENCES public.users(id);


--
-- Name: legacy_actor_mappings legacy_actor_mappings_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legacy_actor_mappings
    ADD CONSTRAINT legacy_actor_mappings_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: legacy_actor_non_identities legacy_actor_non_identities_recorded_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legacy_actor_non_identities
    ADD CONSTRAINT legacy_actor_non_identities_recorded_by_fkey FOREIGN KEY (recorded_by) REFERENCES public.users(id);


--
-- Name: legacy_actor_non_identities legacy_actor_non_identities_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legacy_actor_non_identities
    ADD CONSTRAINT legacy_actor_non_identities_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: metric_snapshots metric_snapshots_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.metric_snapshots
    ADD CONSTRAINT metric_snapshots_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: notifications notifications_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: object_retained_references object_retained_references_storage_key_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.object_retained_references
    ADD CONSTRAINT object_retained_references_storage_key_fkey FOREIGN KEY (storage_key) REFERENCES public.object_lifecycle(storage_key);


--
-- Name: operational_rate_limits operational_rate_limits_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.operational_rate_limits
    ADD CONSTRAINT operational_rate_limits_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE CASCADE;


--
-- Name: operational_rate_limits operational_rate_limits_workflow_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.operational_rate_limits
    ADD CONSTRAINT operational_rate_limits_workflow_id_fkey FOREIGN KEY (workflow_id) REFERENCES public.workflows(id) ON DELETE CASCADE;


--
-- Name: outbox_receipts outbox_receipts_event_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.outbox_receipts
    ADD CONSTRAINT outbox_receipts_event_id_fkey FOREIGN KEY (event_id) REFERENCES public.outbox_events(event_id) ON DELETE CASCADE;


--
-- Name: pmid_import_items pmid_import_items_acquisition_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.pmid_import_items
    ADD CONSTRAINT pmid_import_items_acquisition_run_id_fkey FOREIGN KEY (acquisition_run_id) REFERENCES public.acquisition_runs(id) ON DELETE CASCADE;


--
-- Name: pmid_import_items pmid_import_items_record_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.pmid_import_items
    ADD CONSTRAINT pmid_import_items_record_id_fkey FOREIGN KEY (record_id) REFERENCES public.records(id) ON DELETE SET NULL;


--
-- Name: prisma_snapshots prisma_snapshots_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.prisma_snapshots
    ADD CONSTRAINT prisma_snapshots_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: project_access project_access_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_access
    ADD CONSTRAINT project_access_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: project_access project_access_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_access
    ADD CONSTRAINT project_access_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: project_ai_autonomy project_ai_autonomy_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_ai_autonomy
    ADD CONSTRAINT project_ai_autonomy_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: project_ai_screening_authority project_ai_screening_authority_approved_by_actor_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_ai_screening_authority
    ADD CONSTRAINT project_ai_screening_authority_approved_by_actor_user_id_fkey FOREIGN KEY (approved_by_actor_user_id) REFERENCES public.users(id);


--
-- Name: project_ai_screening_authority project_ai_screening_authority_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_ai_screening_authority
    ADD CONSTRAINT project_ai_screening_authority_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: project_reports project_reports_first_seen_record_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_reports
    ADD CONSTRAINT project_reports_first_seen_record_id_fkey FOREIGN KEY (first_seen_record_id) REFERENCES public.records(id) ON DELETE SET NULL;


--
-- Name: project_reports project_reports_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_reports
    ADD CONSTRAINT project_reports_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: project_reports project_reports_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_reports
    ADD CONSTRAINT project_reports_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: project_unresolved_references project_unresolved_references_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_unresolved_references
    ADD CONSTRAINT project_unresolved_references_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: project_unresolved_references project_unresolved_references_source_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.project_unresolved_references
    ADD CONSTRAINT project_unresolved_references_source_report_id_fkey FOREIGN KEY (source_report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: projects projects_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);


--
-- Name: protocol_versions protocol_versions_amendment_project_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.protocol_versions
    ADD CONSTRAINT protocol_versions_amendment_project_fkey FOREIGN KEY (project_id, amendment_of) REFERENCES public.protocol_versions(project_id, id) ON DELETE CASCADE;


--
-- Name: protocol_versions protocol_versions_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.protocol_versions
    ADD CONSTRAINT protocol_versions_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: record_identifiers record_identifiers_record_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.record_identifiers
    ADD CONSTRAINT record_identifiers_record_id_fkey FOREIGN KEY (record_id) REFERENCES public.records(id) ON DELETE CASCADE;


--
-- Name: records records_acquisition_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.records
    ADD CONSTRAINT records_acquisition_run_id_fkey FOREIGN KEY (acquisition_run_id) REFERENCES public.acquisition_runs(id) ON DELETE SET NULL;


--
-- Name: records records_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.records
    ADD CONSTRAINT records_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: records records_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.records
    ADD CONSTRAINT records_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE SET NULL;


--
-- Name: report_fetch_state report_fetch_state_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_fetch_state
    ADD CONSTRAINT report_fetch_state_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: report_identifiers report_identifiers_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_identifiers
    ADD CONSTRAINT report_identifiers_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: review_artifact_lineage review_artifact_lineage_project_id_artifact_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_artifact_lineage
    ADD CONSTRAINT review_artifact_lineage_project_id_artifact_id_fkey FOREIGN KEY (project_id, artifact_id) REFERENCES public.review_artifacts(project_id, id) ON DELETE CASCADE;


--
-- Name: review_artifact_lineage review_artifact_lineage_project_id_predecessor_artifact_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_artifact_lineage
    ADD CONSTRAINT review_artifact_lineage_project_id_predecessor_artifact_id_fkey FOREIGN KEY (project_id, predecessor_artifact_id) REFERENCES public.review_artifacts(project_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: review_artifacts review_artifacts_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_artifacts
    ADD CONSTRAINT review_artifacts_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: review_attempt_details review_attempt_details_accepted_attempt_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_attempt_details
    ADD CONSTRAINT review_attempt_details_accepted_attempt_id_fkey FOREIGN KEY (accepted_attempt_id) REFERENCES public.durable_attempts(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: review_attempt_details review_attempt_details_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_attempt_details
    ADD CONSTRAINT review_attempt_details_id_fkey FOREIGN KEY (id) REFERENCES public.durable_attempts(id) ON DELETE CASCADE;


--
-- Name: review_attempt_details review_attempt_details_model_run_id_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_attempt_details
    ADD CONSTRAINT review_attempt_details_model_run_id_project_id_fkey FOREIGN KEY (model_run_id, project_id) REFERENCES public.ai_runs(id, project_id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: review_attempt_details review_attempt_details_project_id_accepted_attempt_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_attempt_details
    ADD CONSTRAINT review_attempt_details_project_id_accepted_attempt_id_fkey FOREIGN KEY (project_id, accepted_attempt_id) REFERENCES public.durable_attempts(project_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: review_attempt_details review_attempt_details_project_id_artifact_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_attempt_details
    ADD CONSTRAINT review_attempt_details_project_id_artifact_id_fkey FOREIGN KEY (project_id, artifact_id) REFERENCES public.review_artifacts(project_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: review_attempt_details review_attempt_details_project_id_run_id_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_attempt_details
    ADD CONSTRAINT review_attempt_details_project_id_run_id_id_fkey FOREIGN KEY (project_id, run_id, id) REFERENCES public.durable_attempts(project_id, run_id, id) ON DELETE CASCADE;


--
-- Name: review_attempt_details review_attempt_details_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_attempt_details
    ADD CONSTRAINT review_attempt_details_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.durable_runs(id) ON DELETE CASCADE;


--
-- Name: review_calibration_bundles review_calibration_bundles_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_calibration_bundles
    ADD CONSTRAINT review_calibration_bundles_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: review_events review_events_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_events
    ADD CONSTRAINT review_events_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: review_proposal_finalizations review_proposal_finalizations_model_run_id_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_proposal_finalizations
    ADD CONSTRAINT review_proposal_finalizations_model_run_id_project_id_fkey FOREIGN KEY (model_run_id, project_id) REFERENCES public.ai_runs(id, project_id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: review_proposal_finalizations review_proposal_finalizations_project_id_automation_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_proposal_finalizations
    ADD CONSTRAINT review_proposal_finalizations_project_id_automation_run_id_fkey FOREIGN KEY (project_id, automation_run_id) REFERENCES public.review_run_manifests(project_id, automation_run_id) ON DELETE CASCADE;


--
-- Name: review_proposal_finalizations review_proposal_finalizations_project_id_proposal_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_proposal_finalizations
    ADD CONSTRAINT review_proposal_finalizations_project_id_proposal_id_fkey FOREIGN KEY (project_id, proposal_id) REFERENCES public.ai_proposals(project_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: review_run_manifests review_run_manifests_project_id_automation_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_run_manifests
    ADD CONSTRAINT review_run_manifests_project_id_automation_run_id_fkey FOREIGN KEY (project_id, automation_run_id) REFERENCES public.durable_runs(project_id, id) ON DELETE CASCADE;


--
-- Name: review_run_manifests review_run_manifests_project_id_proposal_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.review_run_manifests
    ADD CONSTRAINT review_run_manifests_project_id_proposal_id_fkey FOREIGN KEY (project_id, proposal_id) REFERENCES public.ai_proposals(project_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: scientific_assignments scientific_assignments_assigned_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.scientific_assignments
    ADD CONSTRAINT scientific_assignments_assigned_by_fkey FOREIGN KEY (assigned_by) REFERENCES public.users(id);


--
-- Name: scientific_assignments scientific_assignments_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.scientific_assignments
    ADD CONSTRAINT scientific_assignments_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: scientific_assignments scientific_assignments_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.scientific_assignments
    ADD CONSTRAINT scientific_assignments_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: screening_events screening_events_actor_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_events
    ADD CONSTRAINT screening_events_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES public.users(id);


--
-- Name: screening_events screening_events_previous_reason_project_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_events
    ADD CONSTRAINT screening_events_previous_reason_project_fk FOREIGN KEY (project_id, previous_full_text_exclusion_reason_id, full_text_reason_stage) REFERENCES public.exclusion_reasons(project_id, id, stage);


--
-- Name: screening_events screening_events_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_events
    ADD CONSTRAINT screening_events_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: screening_events screening_events_project_protocol_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_events
    ADD CONSTRAINT screening_events_project_protocol_fkey FOREIGN KEY (project_id, protocol_version_id) REFERENCES public.protocol_versions(project_id, id) ON DELETE CASCADE;


--
-- Name: screening_events screening_events_reason_project_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_events
    ADD CONSTRAINT screening_events_reason_project_fk FOREIGN KEY (project_id, exclusion_reason_id, stage) REFERENCES public.exclusion_reasons(project_id, id, stage);


--
-- Name: screening_events screening_events_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_events
    ADD CONSTRAINT screening_events_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: screening_events screening_events_result_reason_project_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_events
    ADD CONSTRAINT screening_events_result_reason_project_fk FOREIGN KEY (project_id, result_full_text_exclusion_reason_id, full_text_reason_stage) REFERENCES public.exclusion_reasons(project_id, id, stage);


--
-- Name: screening_events screening_events_supersedes_event_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_events
    ADD CONSTRAINT screening_events_supersedes_event_id_fkey FOREIGN KEY (supersedes_event_id) REFERENCES public.screening_events(id);


--
-- Name: screening_events screening_events_undoes_event_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_events
    ADD CONSTRAINT screening_events_undoes_event_id_fkey FOREIGN KEY (undoes_event_id) REFERENCES public.screening_events(id);


--
-- Name: screening_state screening_state_last_event_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_state
    ADD CONSTRAINT screening_state_last_event_id_fkey FOREIGN KEY (last_event_id) REFERENCES public.screening_events(id);


--
-- Name: screening_state screening_state_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_state
    ADD CONSTRAINT screening_state_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: screening_state screening_state_reason_project_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_state
    ADD CONSTRAINT screening_state_reason_project_fk FOREIGN KEY (project_id, full_text_exclusion_reason_id, full_text_reason_stage) REFERENCES public.exclusion_reasons(project_id, id, stage);


--
-- Name: screening_state screening_state_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.screening_state
    ADD CONSTRAINT screening_state_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: second_review_attempts second_review_attempts_durable_run_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.second_review_attempts
    ADD CONSTRAINT second_review_attempts_durable_run_fk FOREIGN KEY (project_id, review_run_id) REFERENCES public.durable_runs(project_id, id);


--
-- Name: second_review_attempts second_review_attempts_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.second_review_attempts
    ADD CONSTRAINT second_review_attempts_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: second_review_attempts second_review_attempts_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.second_review_attempts
    ADD CONSTRAINT second_review_attempts_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: second_review_gate second_review_gate_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.second_review_gate
    ADD CONSTRAINT second_review_gate_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: studies studies_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.studies
    ADD CONSTRAINT studies_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: study_events study_events_project_id_before_study_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_events
    ADD CONSTRAINT study_events_project_id_before_study_id_fkey FOREIGN KEY (project_id, before_study_id) REFERENCES public.studies(project_id, id) ON DELETE CASCADE;


--
-- Name: study_events study_events_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_events
    ADD CONSTRAINT study_events_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: study_events study_events_project_id_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_events
    ADD CONSTRAINT study_events_project_id_report_id_fkey FOREIGN KEY (project_id, report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: study_events study_events_project_id_result_study_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_events
    ADD CONSTRAINT study_events_project_id_result_study_id_fkey FOREIGN KEY (project_id, result_study_id) REFERENCES public.studies(project_id, id) ON DELETE CASCADE;


--
-- Name: study_events study_events_project_id_study_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_events
    ADD CONSTRAINT study_events_project_id_study_id_fkey FOREIGN KEY (project_id, study_id) REFERENCES public.studies(project_id, id) ON DELETE CASCADE;


--
-- Name: study_reports study_reports_report_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_reports
    ADD CONSTRAINT study_reports_report_id_fkey FOREIGN KEY (report_id) REFERENCES public.reports(id) ON DELETE CASCADE;


--
-- Name: study_reports study_reports_report_project_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_reports
    ADD CONSTRAINT study_reports_report_project_fk FOREIGN KEY (project_id, report_id) REFERENCES public.project_reports(project_id, report_id) ON DELETE CASCADE;


--
-- Name: study_reports study_reports_study_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_reports
    ADD CONSTRAINT study_reports_study_id_fkey FOREIGN KEY (study_id) REFERENCES public.studies(id) ON DELETE CASCADE;


--
-- Name: study_reports study_reports_study_project_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.study_reports
    ADD CONSTRAINT study_reports_study_project_fk FOREIGN KEY (project_id, study_id) REFERENCES public.studies(project_id, id) ON DELETE CASCADE;


--
-- Name: transport_dead_letters transport_dead_letters_event_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.transport_dead_letters
    ADD CONSTRAINT transport_dead_letters_event_id_fkey FOREIGN KEY (event_id) REFERENCES public.outbox_events(event_id) ON DELETE CASCADE;


--
-- Name: user_sessions user_sessions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: workflow_files workflow_files_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_files
    ADD CONSTRAINT workflow_files_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: workflow_files workflow_files_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_files
    ADD CONSTRAINT workflow_files_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.durable_runs(id) ON DELETE CASCADE;


--
-- Name: workflow_run_details workflow_run_details_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_run_details
    ADD CONSTRAINT workflow_run_details_id_fkey FOREIGN KEY (id) REFERENCES public.durable_runs(id) ON DELETE CASCADE;


--
-- Name: workflow_run_details workflow_run_details_project_id_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_run_details
    ADD CONSTRAINT workflow_run_details_project_id_id_fkey FOREIGN KEY (project_id, id) REFERENCES public.durable_runs(project_id, id) ON DELETE CASCADE;


--
-- Name: workflow_run_details workflow_run_details_project_id_workflow_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_run_details
    ADD CONSTRAINT workflow_run_details_project_id_workflow_id_fkey FOREIGN KEY (project_id, workflow_id) REFERENCES public.workflows(project_id, id) ON DELETE CASCADE;


--
-- Name: workflow_run_details workflow_run_details_workflow_id_version_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_run_details
    ADD CONSTRAINT workflow_run_details_workflow_id_version_id_fkey FOREIGN KEY (workflow_id, version_id) REFERENCES public.workflow_versions(workflow_id, id) ON DELETE CASCADE;


--
-- Name: workflow_secrets workflow_secrets_workflow_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_secrets
    ADD CONSTRAINT workflow_secrets_workflow_id_fkey FOREIGN KEY (workflow_id) REFERENCES public.workflows(id) ON DELETE CASCADE;


--
-- Name: workflow_seen_items workflow_seen_items_workflow_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_seen_items
    ADD CONSTRAINT workflow_seen_items_workflow_id_fkey FOREIGN KEY (workflow_id) REFERENCES public.workflows(id) ON DELETE CASCADE;


--
-- Name: workflow_step_details workflow_step_details_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_step_details
    ADD CONSTRAINT workflow_step_details_id_fkey FOREIGN KEY (id) REFERENCES public.durable_steps(id) ON DELETE CASCADE;


--
-- Name: workflow_step_details workflow_step_details_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_step_details
    ADD CONSTRAINT workflow_step_details_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.durable_runs(id) ON DELETE CASCADE;


--
-- Name: workflow_step_details workflow_step_details_run_id_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_step_details
    ADD CONSTRAINT workflow_step_details_run_id_id_fkey FOREIGN KEY (run_id, id) REFERENCES public.durable_steps(run_id, id) ON DELETE CASCADE;


--
-- Name: workflow_versions workflow_versions_project_id_workflow_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflow_versions
    ADD CONSTRAINT workflow_versions_project_id_workflow_id_fkey FOREIGN KEY (project_id, workflow_id) REFERENCES public.workflows(project_id, id) ON DELETE CASCADE;


--
-- Name: workflows workflows_project_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflows
    ADD CONSTRAINT workflows_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;


--
-- Name: workflows workflows_published_version_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workflows
    ADD CONSTRAINT workflows_published_version_fk FOREIGN KEY (id, published_version_id) REFERENCES public.workflow_versions(workflow_id, id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: workspace_invitations workspace_invitations_accepted_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_invitations
    ADD CONSTRAINT workspace_invitations_accepted_by_user_id_fkey FOREIGN KEY (accepted_by_user_id) REFERENCES public.users(id);


--
-- Name: workspace_invitations workspace_invitations_invited_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_invitations
    ADD CONSTRAINT workspace_invitations_invited_by_fkey FOREIGN KEY (invited_by) REFERENCES public.users(id);


--
-- Name: workspace_invitations workspace_invitations_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_invitations
    ADD CONSTRAINT workspace_invitations_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);


--
-- Name: workspace_memberships workspace_memberships_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_memberships
    ADD CONSTRAINT workspace_memberships_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);


--
-- Name: workspace_memberships workspace_memberships_workspace_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_memberships
    ADD CONSTRAINT workspace_memberships_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id);


--
-- Name: FUNCTION activate_account_erasure(p_attestation text); Type: ACL; Schema: public; Owner: -
--

REVOKE ALL ON FUNCTION public.activate_account_erasure(p_attestation text) FROM PUBLIC;


--
-- Name: FUNCTION activate_export_retention(p_attestation text); Type: ACL; Schema: public; Owner: -
--

REVOKE ALL ON FUNCTION public.activate_export_retention(p_attestation text) FROM PUBLIC;


--
-- Name: FUNCTION billing_preserve_closure_context(); Type: ACL; Schema: public; Owner: -
--

REVOKE ALL ON FUNCTION public.billing_preserve_closure_context() FROM PUBLIC;


--
-- Name: FUNCTION configure_database_roles(p_runtime text, p_migration text, p_maintenance text); Type: ACL; Schema: public; Owner: -
--

REVOKE ALL ON FUNCTION public.configure_database_roles(p_runtime text, p_migration text, p_maintenance text) FROM PUBLIC;


--
-- Name: FUNCTION enforce_account_physical_proof(); Type: ACL; Schema: public; Owner: -
--

REVOKE ALL ON FUNCTION public.enforce_account_physical_proof() FROM PUBLIC;


--
-- Name: FUNCTION enforce_object_retention_target(); Type: ACL; Schema: public; Owner: -
--

REVOKE ALL ON FUNCTION public.enforce_object_retention_target() FROM PUBLIC;


--
-- Name: FUNCTION invalidate_inactivity_contact(); Type: ACL; Schema: public; Owner: -
--

REVOKE ALL ON FUNCTION public.invalidate_inactivity_contact() FROM PUBLIC;


--
-- Name: FUNCTION purge_retention_records(p_batch integer); Type: ACL; Schema: public; Owner: -
--

REVOKE ALL ON FUNCTION public.purge_retention_records(p_batch integer) FROM PUBLIC;


--
-- Name: FUNCTION redact_deleted_user_audit(p_user_id uuid); Type: ACL; Schema: public; Owner: -
--

REVOKE ALL ON FUNCTION public.redact_deleted_user_audit(p_user_id uuid) FROM PUBLIC;


--
-- Name: FUNCTION reject_admin_audit_event_changes(); Type: ACL; Schema: public; Owner: -
--

REVOKE ALL ON FUNCTION public.reject_admin_audit_event_changes() FROM PUBLIC;


--
-- PostgreSQL database dump complete
--
