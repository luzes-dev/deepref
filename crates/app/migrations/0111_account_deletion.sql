-- phase: expand
-- "Delete my account" (11, BUSINESS_DECISIONS). A deletion request is
-- recorded on the user row and completed 30 days later by
-- `deepref-server accounts complete-deletions`, which anonymizes the account
-- and logs the fulfillment.
--
-- `users.deletion_requested_at` is the pending request (NULL when none).
-- `fulfilled_account_deletions` is the log of completed deletions: user id and
-- completion time only, no content, kept five years per BUSINESS_DECISIONS.
-- Restores replay it so a restored backup never brings deleted personal data
-- back (docs/security.md, "Restore step").
ALTER TABLE users ADD COLUMN deletion_requested_at timestamptz;

CREATE TABLE fulfilled_account_deletions (
  user_id uuid PRIMARY KEY REFERENCES users(id),
  completed_at timestamptz NOT NULL DEFAULT now()
);

-- The completion command scans for requests that have aged out their 30 days.
CREATE INDEX users_deletion_requested_idx
  ON users (deletion_requested_at)
  WHERE deletion_requested_at IS NOT NULL;
