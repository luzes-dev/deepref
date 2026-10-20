-- phase: expand
-- Retention purge gate (11 Part B, BUSINESS_DECISIONS: administrative and
-- security audit rows are kept 6 months; the record that a deletion request
-- was fulfilled is kept 5 years without content).
--
-- The append-only trigger keeps refusing every UPDATE, every DELETE outside
-- the purge, and every TRUNCATE. The one narrow exception is the operator
-- purge (`deepref-server audit purge`), which opens with
-- `SET LOCAL deepref.audit_purge = 'on'` in the deleting transaction. Even
-- then the trigger itself re-checks the retention cutoff row by row: a row
-- inside its retention window cannot be deleted by any caller, in any
-- transaction, and the setting does not survive its transaction (SET LOCAL),
-- so it cannot be left on by accident.
--
-- Cutoffs live in the trigger and in `deepref_app::audit_admin` together; the
-- purge command tests assert they agree.
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
  RAISE EXCEPTION 'administrative audit events are append-only'
    USING ERRCODE = 'integrity_constraint_violation';
END;
$$;
