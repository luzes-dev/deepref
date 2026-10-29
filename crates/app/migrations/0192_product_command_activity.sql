-- phase: expand
-- Expand provenance only. No activity is inferred or backfilled from existing
-- product rows, worker events, profile timestamps or session refreshes.
ALTER TABLE account_human_activity
  DROP CONSTRAINT account_human_activity_evidence_kind_check,
  ADD CONSTRAINT account_human_activity_evidence_kind_check
    CHECK(evidence_kind IN ('sign_in','browser_interaction','product_command'));
