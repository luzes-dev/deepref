-- phase: expand
-- Human evidence starts with an actual successful sign-in or trusted browser
-- interaction. Existing profile/session/job timestamps are never backfilled.
CREATE TABLE account_human_activity (
  user_id uuid PRIMARY KEY REFERENCES users(id),
  generation uuid NOT NULL,
  contact_generation uuid NOT NULL DEFAULT gen_random_uuid(),
  occurred_at timestamptz NOT NULL,
  evidence_kind text NOT NULL CHECK(evidence_kind IN ('sign_in','browser_interaction'))
);

-- Address changes revoke warning authority without claiming human activity.
-- Changing away and back cannot resurrect a receipt for an earlier contact.
CREATE FUNCTION invalidate_inactivity_contact() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public AS $$
BEGIN
  UPDATE public.account_human_activity SET contact_generation=gen_random_uuid()
    WHERE user_id=NEW.id;
  RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION invalidate_inactivity_contact() FROM PUBLIC;
CREATE TRIGGER account_inactivity_contact_changed AFTER UPDATE OF verified_email ON users
  FOR EACH ROW WHEN(OLD.verified_email IS DISTINCT FROM NEW.verified_email)
  EXECUTE FUNCTION invalidate_inactivity_contact();
