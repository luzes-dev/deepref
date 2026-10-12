-- ADR 0005 §2.4: a semantic identity change voids open cohorts but only flags
-- finalized ones for review; only a protocol republish (§13) reopens finalized
-- automation exclusions. The flag is review metadata, never sample design.
ALTER TABLE ai_screening_cohorts
  ADD COLUMN identity_flagged_at timestamptz,
  ADD COLUMN identity_flag_reason text,
  ADD CONSTRAINT ai_screening_cohorts_identity_flag_check
    CHECK ((identity_flagged_at IS NULL) = (identity_flag_reason IS NULL));

-- Same rules as 0054, with the flag columns allowed to change after the draw.
CREATE OR REPLACE FUNCTION protect_ai_first_evidence() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='ai_screening_audit_labels' THEN
   RAISE EXCEPTION 'audit labels are immutable';
 ELSIF TG_TABLE_NAME='ai_screening_dispositions' THEN
   IF (to_jsonb(NEW)-ARRAY['voided_at','void_reason','finalized_at','finalized_event_id'])
      IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['voided_at','void_reason','finalized_at','finalized_event_id'])
      OR OLD.voided_at IS NOT NULL OR OLD.finalized_at IS NOT NULL THEN
     RAISE EXCEPTION 'disposition evidence is immutable';
   END IF;
 ELSIF OLD.sampling_seed IS NOT NULL AND
   (to_jsonb(NEW)-ARRAY['status','result','evaluated_at','final_approval_id','finalized_by','finalized_at','invalidation_reason','identity_flagged_at','identity_flag_reason'])
   IS DISTINCT FROM
   (to_jsonb(OLD)-ARRAY['status','result','evaluated_at','final_approval_id','finalized_by','finalized_at','invalidation_reason','identity_flagged_at','identity_flag_reason']) THEN
   RAISE EXCEPTION 'sample design is frozen';
 END IF;
 RETURN NEW;
END $$;
