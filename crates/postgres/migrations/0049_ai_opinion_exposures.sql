-- AI opinion exposures: an append-only record of when an AI screening opinion
-- about one record and stage became available to a person.
--
-- Second-reviewer independence means the person decided without having seen
-- the AI's verdict. Whether that held is a fact about the time of each view,
-- so it is recorded here rather than inferred later. A pair (human decision,
-- AI opinion) is exposed when an exposure exists for the same record and stage
-- at or before the human decision (see crates/postgres/src/ai_exposure.rs).
--
-- Exposures are keyed by record and stage, not by AI run, on purpose. Opinions
-- from the same model on the same record are correlated, so any AI verdict a
-- person saw first contaminates a later independent-looking decision.

CREATE TABLE ai_opinion_exposures (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  report_id uuid NOT NULL,
  stage text NOT NULL CHECK (stage IN ('title_abstract', 'full_text')),
  -- Which route made the AI verdict available:
  --   workflow_run_output     a workflow screening step attached the verdict to
  --                           its records, or a second-opinion step put it in
  --                           its run output. Visible in run inspection and
  --                           carried to notifications and integrations.
  --   screening_suggestion    a suggest-mode screening proposal was listed or
  --                           fetched, so the screening page could show it.
  --   reviewer_opinion_reveal the second reviewer's opinion was returned after
  --                           the person decided: by the reviewer-decision API
  --                           (listing or resolving), or by the activity feed.
  --                           Later changes of decision are measured against it.
  --   activity_feed_legacy    the activity feed showed the opinion before
  --                           blinding existed. Backfilled from
  --                           ai_reviewer_decisions.created_at.
  --   proposal_legacy         a screening proposal was listable before blinding
  --                           existed. Backfilled from ai_proposals.created_at.
  exposure_source text NOT NULL CHECK (exposure_source IN (
    'workflow_run_output', 'screening_suggestion', 'reviewer_opinion_reveal',
    'activity_feed_legacy', 'proposal_legacy'
  )),
  -- NULL means anyone with access to the project saw it. Set only when a route
  -- showed the verdict to one named actor. No such route exists yet.
  audience_actor_kind text CHECK (audience_actor_kind IS NULL OR audience_actor_kind IN (
    'user', 'automation', 'system'
  )),
  audience_actor_id text CHECK (audience_actor_id IS NULL OR length(btrim(audience_actor_id)) > 0),
  -- ON DELETE CASCADE, not SET NULL: SET NULL would UPDATE this append-only
  -- table, which the trigger below refuses, and project deletion would fail.
  ai_reviewer_decision_id uuid REFERENCES ai_reviewer_decisions(id) ON DELETE CASCADE,
  proposal_id uuid,
  ai_run_id uuid,
  workflow_run_id uuid,
  exposure_possible_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (project_id, report_id) REFERENCES project_reports(project_id, report_id) ON DELETE CASCADE,
  CONSTRAINT ai_opinion_exposures_audience_shape CHECK (
    (audience_actor_kind IS NULL AND audience_actor_id IS NULL)
    OR (audience_actor_kind IS NOT NULL AND audience_actor_id IS NOT NULL)
  )
);

CREATE INDEX ai_opinion_exposures_lookup_idx
  ON ai_opinion_exposures (project_id, report_id, stage, exposure_possible_at);

-- Recording is idempotent where an identifier exists. A reveal is recorded once
-- per decision, and a proposal-based exposure once per proposal and source. The
-- earliest time is the one that matters, so later repeats are ignored.
CREATE UNIQUE INDEX ai_opinion_exposures_decision_source_uq
  ON ai_opinion_exposures (ai_reviewer_decision_id, exposure_source,
      COALESCE(audience_actor_kind, ''), COALESCE(audience_actor_id, ''))
  WHERE ai_reviewer_decision_id IS NOT NULL;
CREATE UNIQUE INDEX ai_opinion_exposures_proposal_source_uq
  ON ai_opinion_exposures (proposal_id, exposure_source,
      COALESCE(audience_actor_kind, ''), COALESCE(audience_actor_id, ''))
  WHERE proposal_id IS NOT NULL AND ai_reviewer_decision_id IS NULL;

CREATE OR REPLACE FUNCTION reject_ai_opinion_exposure_changes()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  RAISE EXCEPTION 'AI opinion exposures are append-only'
    USING ERRCODE = 'integrity_constraint_violation';
END;
$$;

-- Only UPDATE is refused. DELETE stays allowed so that a project or report
-- delete can cascade through this table.
CREATE TRIGGER ai_opinion_exposure_append_only_trigger
BEFORE UPDATE ON ai_opinion_exposures
FOR EACH ROW EXECUTE FUNCTION reject_ai_opinion_exposure_changes();

-- The human decision that resolve_reviewer_conflict replaced. The independence
-- check needs its time, which is the latest user screening event at resolution.
ALTER TABLE ai_reviewer_decisions
  ADD COLUMN human_decision_before_at timestamptz;

-- Looks up the second-reviewer decision behind a screening proposal, so a
-- waiting opinion's proposal can be withheld from the proposal API.
CREATE INDEX ai_reviewer_decisions_proposal_idx
  ON ai_reviewer_decisions (project_id, proposal_id)
  WHERE proposal_id IS NOT NULL;

-- Legacy backfill. This states what was true, and nothing more.
--
-- The activity feed showed every AI second-reviewer opinion from the moment its
-- row was written. The decision row and its activity entry are written in one
-- transaction, so created_at is the time the opinion became visible. Voided
-- decisions were visible too, so they are included. Entries whose decision row
-- is missing cannot be keyed to a record and stage, and so have no exposure.
INSERT INTO ai_opinion_exposures
  (project_id, report_id, stage, exposure_source, ai_reviewer_decision_id,
   proposal_id, ai_run_id, exposure_possible_at)
SELECT d.project_id, d.report_id, d.stage, 'activity_feed_legacy', d.id,
       d.proposal_id, d.ai_run_id, d.created_at
FROM ai_reviewer_decisions d;

-- Screening proposals were listable from creation, expired or pending. The
-- stage is read from the payload, where screening proposals store it, and the
-- record from target_report_id. A row without a stage in the payload or
-- without a target report cannot be keyed to a record and stage, so it is
-- skipped rather than guessed. Its payload is not read further.
INSERT INTO ai_opinion_exposures
  (project_id, report_id, stage, exposure_source, proposal_id, ai_run_id, exposure_possible_at)
SELECT p.project_id, p.target_report_id, p.payload->>'stage', 'proposal_legacy', p.id,
       p.model_run_id, p.created_at
FROM ai_proposals p
WHERE p.operation = 'screening_suggestion'
  AND p.target_report_id IS NOT NULL
  AND p.payload->>'stage' IN ('title_abstract', 'full_text');
