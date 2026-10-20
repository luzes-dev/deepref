-- phase: expand
-- Retention purge gate (11 Part B, BUSINESS_DECISIONS: administrative and
-- security audit rows are kept 6 months; the record that a deletion request
-- was fulfilled is kept 5 years without content).
--
-- The append-only trigger keeps refusing every UPDATE, every DELETE outside
-- the gates below, and every TRUNCATE. It honours exactly two transaction-local
-- settings (`SET LOCAL`, so neither can be left on by accident):
--
--   deepref.audit_purge = 'on'      the operator purge
--                                   (`deepref-server audit purge`). Even then
--                                   the trigger itself re-checks the retention
--                                   cutoff row by row: a row inside its
--                                   retention window cannot be deleted by any
--                                   caller, in any transaction.
--   deepref.audit_redact = <uuid>   PII redaction for exactly that one user
--                                   (account deletion completion). Only the
--                                   named PII keys of that user's
--                                   `legacy_actor.*` rows may change, and only
--                                   to the fixed `redacted` tombstone: the
--                                   event id, type and every identifier stay
--                                   (docs/security.md, "PII erasure").
--
-- Cutoffs live in the trigger and in `deepref_app::audit_admin` together; the
-- purge command tests assert they agree. The named redaction keys live in the
-- trigger and in `deepref_app::account_deletion` together; the deletion tests
-- assert those agree too.
CREATE OR REPLACE FUNCTION reject_admin_audit_event_changes()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  IF TG_OP = 'DELETE' AND current_setting('deepref.audit_purge', true) = 'on' THEN
    IF OLD.event_type = 'account.deletion_completed' THEN
      -- The proof that a deletion request was fulfilled (BUSINESS_DECISIONS).
      IF OLD.occurred_at < now() - interval '5 years' THEN
        RETURN OLD;
      END IF;
    ELSIF OLD.occurred_at < now() - interval '6 months' THEN
      RETURN OLD;
    END IF;
  END IF;
  IF TG_OP = 'UPDATE' AND coalesce(current_setting('deepref.audit_redact', true), '') <> '' THEN
    -- Named-key PII redaction for one user (11 Part B). Everything that
    -- identifies the event or the tenancy is immutable; only the two free-text
    -- label keys of a legacy-identity decision may move to the tombstone.
    IF OLD.event_type IN ('legacy_actor.mapped', 'legacy_actor.non_identity_recorded')
       AND OLD.target_user_id IS NOT NULL
       AND OLD.target_user_id::text = current_setting('deepref.audit_redact', true)
       AND NEW.id = OLD.id
       AND NEW.occurred_at = OLD.occurred_at
       AND NEW.event_type = OLD.event_type
       AND NEW.principal_kind = OLD.principal_kind
       AND NEW.principal_user_id IS NOT DISTINCT FROM OLD.principal_user_id
       AND NEW.target_user_id IS NOT DISTINCT FROM OLD.target_user_id
       AND NEW.workspace_id IS NOT DISTINCT FROM OLD.workspace_id
       AND NEW.project_id IS NOT DISTINCT FROM OLD.project_id
       AND NEW.session_ref IS NOT DISTINCT FROM OLD.session_ref
       AND NEW.request_id IS NOT DISTINCT FROM OLD.request_id
       AND NEW.metadata - 'legacy_actor_id' - 'source'
           = OLD.metadata - 'legacy_actor_id' - 'source'
       AND NEW.metadata->>'legacy_actor_id' = 'redacted'
       AND NEW.metadata->>'source' = 'redacted'
    THEN
      RETURN NEW;
    END IF;
  END IF;
  RAISE EXCEPTION 'administrative audit events are append-only'
    USING ERRCODE = 'integrity_constraint_violation';
END;
$$;
