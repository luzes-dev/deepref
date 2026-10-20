-- phase: expand
-- "Delete my account" (11, BUSINESS_DECISIONS). A deletion request is an
-- explicit state machine, and completion runs 30 days later via
-- `deepref-server accounts complete-deletions`, which anonymizes the account
-- and logs the fulfillment.
--
-- `account_deletion_requests` is the state machine: one row per request, in
-- exactly one of `requested`, `cancelled`, `completing`, `completed`. The
-- request row (not the user row) is what completion and cancellation lock, so
-- the two can never interleave: cancellation succeeds only from `requested`,
-- and completion claims a due request by moving it to `completing` before any
-- destructive work and to `completed` in the transaction that finishes it. A
-- request is therefore never due twice, whatever happens to the proof row.
--
-- `users.deleted_at` (0085) is the durable anonymization marker. It is set at
-- completion and never cleared; the BEFORE UPDATE trigger below keeps the
-- anonymized form of the account whatever any later UPDATE says. That covers
-- N-1 sign-in SQL and restores at the database boundary, independently of the
-- compliance log below (which is purged after five years).
--
-- `fulfilled_account_deletions` is the log of completed deletions: user id and
-- completion time only, no content, kept five years per BUSINESS_DECISIONS.
-- Restores replay it so a restored backup never brings deleted personal data
-- back (docs/security.md, "Restore step").
CREATE TABLE account_deletion_requests (
  id uuid PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users (id),
  state text NOT NULL CHECK (state IN ('requested', 'cancelled', 'completing', 'completed')),
  requested_at timestamptz NOT NULL DEFAULT now(),
  resolved_at timestamptz,
  -- The clock runs only while the request is open or being completed.
  CHECK ((state IN ('requested', 'completing')) = (resolved_at IS NULL))
);

-- At most one open request per user: a fresh request after a cancellation is
-- a new row, and its 30-day wait starts again.
CREATE UNIQUE INDEX account_deletion_requests_open_idx
  ON account_deletion_requests (user_id)
  WHERE state IN ('requested', 'completing');

-- The completion scan looks for requests that have aged out their 30 days.
CREATE INDEX account_deletion_requests_due_idx
  ON account_deletion_requests (requested_at)
  WHERE state = 'requested';

CREATE TABLE fulfilled_account_deletions (
  user_id uuid PRIMARY KEY REFERENCES users(id),
  completed_at timestamptz NOT NULL DEFAULT now()
);

-- The durable anonymization guard. Once `users.deleted_at` is set, no UPDATE
-- can put personal data back or un-delete the account: the anonymized values
-- are re-asserted whatever the statement says. The form is deliberately
-- identical to what `deepref_app::account_deletion` writes at completion
-- (`FORMER_MEMBER_LABEL`, and the WorkOS link tombstoned as `deleted:<user
-- uuid>` so the unique constraint holds and a later sign-in by the same person
-- resolves to a NEW user instead of resurrecting this one).
CREATE FUNCTION enforce_anonymized_deleted_user() RETURNS trigger
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

CREATE TRIGGER users_anonymized_account_guard
BEFORE UPDATE ON users
FOR EACH ROW EXECUTE FUNCTION enforce_anonymized_deleted_user();
