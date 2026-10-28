-- phase: expand
-- Correct the role contract without rewriting applied 0187/0188. This does
-- not activate or widen credentials: explicit administrator adoption remains
-- required. Canonical protected bodies retain their existing semantics.
CREATE OR REPLACE FUNCTION public.reject_admin_audit_event_changes()
RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog AS $$
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

CREATE OR REPLACE FUNCTION public.purge_retention_records(p_batch integer)
RETURNS TABLE(batches integer,audit_rows bigint,fulfilled_rows bigint,session_rows bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='250ms' AS $$
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

CREATE OR REPLACE FUNCTION public.redact_deleted_user_audit(p_user_id uuid)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog
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

CREATE OR REPLACE TRIGGER admin_audit_events_append_only_trigger
BEFORE UPDATE OR DELETE ON public.admin_audit_events
FOR EACH ROW EXECUTE FUNCTION public.reject_admin_audit_event_changes();
CREATE OR REPLACE TRIGGER admin_audit_events_no_truncate_trigger
BEFORE TRUNCATE ON public.admin_audit_events
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_admin_audit_event_changes();
ALTER TABLE public.admin_audit_events ENABLE TRIGGER admin_audit_events_append_only_trigger;
ALTER TABLE public.admin_audit_events ENABLE TRIGGER admin_audit_events_no_truncate_trigger;

CREATE OR REPLACE FUNCTION public.configure_database_roles(p_runtime text,p_migration text,p_maintenance text)
RETURNS void LANGUAGE plpgsql SET search_path=pg_catalog AS $$
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
    IF entry.relname IN('_sqlx_migrations','database_role_contract') THEN
      EXECUTE format('REVOKE INSERT (%1$s), UPDATE (%1$s) ON TABLE public.%2$I FROM deepref_runtime',entry.column_names,entry.relname);
    END IF;
    IF entry.relname IN('admin_audit_events','fulfilled_account_deletions','user_sessions','database_role_contract','_sqlx_migrations') THEN
      EXECUTE format('REVOKE SELECT (%1$s), INSERT (%1$s), UPDATE (%1$s) ON TABLE public.%2$I FROM deepref_maintenance',entry.column_names,entry.relname);
    END IF;
  END LOOP;
  REVOKE TRUNCATE,REFERENCES,TRIGGER ON ALL TABLES IN SCHEMA public FROM deepref_runtime,deepref_maintenance;
  REVOKE ALL ON public._sqlx_migrations,public.database_role_contract FROM deepref_runtime;
  GRANT SELECT ON public._sqlx_migrations,public.database_role_contract TO deepref_runtime;
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
$$;
REVOKE ALL ON FUNCTION public.configure_database_roles(text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.purge_retention_records(integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.redact_deleted_user_audit(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.reject_admin_audit_event_changes() FROM PUBLIC;
