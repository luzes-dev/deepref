-- Canonical protected function. This resource is also the trusted drift-repair fixture.
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
REVOKE ALL ON FUNCTION public.purge_retention_records(integer) FROM PUBLIC;
