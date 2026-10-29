-- phase: expand
-- Compose approved privacy activation with the restricted runtime/maintenance
-- contract. Existing deletion business journals/maps keep ordinary runtime DML.
-- No attestation is inferred, and this migration activates neither rollout.
CREATE OR REPLACE FUNCTION public.activate_export_retention(p_attestation text)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER
SET search_path=pg_catalog SET lock_timeout='250ms' SET statement_timeout='1500ms' AS $$
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
REVOKE ALL ON FUNCTION public.activate_export_retention(text) FROM PUBLIC;
-- Preserve the exact reviewed0189 eligibility body and attestation. Bound its
-- SQL effects independently of the caller's session settings.
ALTER FUNCTION public.activate_account_erasure(text) SET lock_timeout='250ms';
ALTER FUNCTION public.activate_account_erasure(text) SET statement_timeout='1500ms';

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
$$;
REVOKE ALL ON FUNCTION public.configure_database_roles(text,text,text) FROM PUBLIC;
REVOKE ALL ON FUNCTION public.activate_account_erasure(text) FROM PUBLIC;

-- Also apply the narrow boundary on an already-activated installation. Full
-- direct LOGIN cleanup/adoption remains the explicit administrator command.
REVOKE ALL ON public.account_export_rollout,public.account_erasure_rollout FROM PUBLIC;
DO $$
DECLARE entry record;
BEGIN
  IF EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname='deepref_runtime') THEN
    REVOKE ALL ON public.account_export_rollout,public.account_erasure_rollout FROM deepref_runtime;
    FOR entry IN SELECT c.relname,string_agg(format('%I',a.attname),',' ORDER BY a.attnum) AS cols
      FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
      JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
      WHERE n.nspname='public' AND c.relname IN('account_export_rollout','account_erasure_rollout')
      GROUP BY c.oid,c.relname LOOP
      EXECUTE format('REVOKE SELECT (%1$s), INSERT (%1$s), UPDATE (%1$s), REFERENCES (%1$s) ON public.%2$I FROM PUBLIC',entry.cols,entry.relname);
      EXECUTE format('REVOKE INSERT (%1$s), UPDATE (%1$s), REFERENCES (%1$s) ON public.%2$I FROM deepref_runtime',entry.cols,entry.relname);
    END LOOP;
    GRANT SELECT ON public.account_export_rollout,public.account_erasure_rollout TO deepref_runtime;
    REVOKE ALL ON FUNCTION public.activate_export_retention(text),public.activate_account_erasure(text) FROM deepref_runtime;
  END IF;
  IF EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname='deepref_maintenance') THEN
    REVOKE ALL ON public.account_export_rollout,public.account_erasure_rollout FROM deepref_maintenance;
    FOR entry IN SELECT c.relname,string_agg(format('%I',a.attname),',' ORDER BY a.attnum) AS cols
      FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
      JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
      WHERE n.nspname='public' AND c.relname IN('account_export_rollout','account_erasure_rollout')
      GROUP BY c.oid,c.relname LOOP
      EXECUTE format('REVOKE SELECT (%1$s), INSERT (%1$s), UPDATE (%1$s), REFERENCES (%1$s) ON public.%2$I FROM deepref_maintenance',entry.cols,entry.relname);
    END LOOP;
    GRANT EXECUTE ON FUNCTION public.activate_export_retention(text),public.activate_account_erasure(text) TO deepref_maintenance;
  END IF;
END;
$$;
