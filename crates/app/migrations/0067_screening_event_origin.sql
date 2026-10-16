-- Provenance for screening events: who authored the decision, not only who
-- recorded it. A confirmed assistant plan writes under the confirming user's
-- actor, but the decision itself was chosen by a model. `assistant_plan`
-- marks those events so PRISMA and the ADR 0005 human reference stratum can
-- tell them apart from a person's own screening.
--
-- NULL means a direct decision recorded before this column existed, and
-- 'direct' may be written explicitly; both read as a person's own decision.
ALTER TABLE screening_events
  ADD COLUMN origin text
  CONSTRAINT screening_events_origin_check
  CHECK (origin IS NULL OR origin IN ('direct', 'assistant_plan'));

-- Assistant-plan events stay a small minority; a partial index keeps the
-- "is this canonical decision assistant-authored" lookups cheap.
CREATE INDEX screening_events_assistant_plan_idx
  ON screening_events (project_id, report_id)
  WHERE origin = 'assistant_plan';
