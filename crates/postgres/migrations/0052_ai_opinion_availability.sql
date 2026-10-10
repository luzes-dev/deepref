-- Record availability rather than page opens. Pending proposals in second-reviewer
-- mode are blinded even before the worker creates their reviewer-decision row.
CREATE FUNCTION ai_screening_proposal_withheld(
    project uuid, proposal uuid, report uuid, screening_stage text
) RETURNS boolean LANGUAGE sql STABLE AS $$
    SELECT COALESCE(screening_stage IN ('title_abstract', 'full_text'), false) AND (
      EXISTS (
        SELECT 1 FROM ai_reviewer_decisions d
        LEFT JOIN screening_state s ON s.project_id=d.project_id AND s.report_id=d.report_id
        WHERE d.project_id=project AND d.proposal_id=proposal AND d.resolved_at IS NULL
          AND COALESCE(CASE WHEN d.stage='full_text' THEN s.full_text_status
                           ELSE s.title_abstract_status END, 'unscreened')
              IN ('unscreened', 'not_required')
      ) OR (
        COALESCE((SELECT a.level FROM project_ai_autonomy a
                  WHERE a.project_id=project AND a.task=screening_stage || '_screening'),
                 'second_reviewer')='second_reviewer'
        AND COALESCE((SELECT CASE WHEN screening_stage='full_text' THEN s.full_text_status
                                  ELSE s.title_abstract_status END
                      FROM screening_state s WHERE s.project_id=project AND s.report_id=report),
                     'unscreened') IN ('unscreened', 'not_required')
      )
    )
$$;

CREATE FUNCTION record_available_ai_opinions(project uuid, report uuid)
RETURNS void LANGUAGE sql AS $$
    INSERT INTO ai_opinion_exposures
      (project_id, report_id, stage, exposure_source, proposal_id, ai_run_id, exposure_possible_at)
    SELECT p.project_id, p.target_report_id, p.payload->>'stage', 'screening_suggestion',
           p.id, p.model_run_id, clock_timestamp()
    FROM ai_proposals p
    WHERE p.project_id=project AND (report IS NULL OR p.target_report_id=report)
      AND p.operation='screening_suggestion' AND p.target_report_id IS NOT NULL
      AND p.payload->>'stage' IN ('title_abstract', 'full_text')
      AND NOT ai_screening_proposal_withheld(p.project_id,p.id,p.target_report_id,p.payload->>'stage')
    ON CONFLICT DO NOTHING;

    INSERT INTO ai_opinion_exposures
      (project_id, report_id, stage, exposure_source, ai_reviewer_decision_id,
       proposal_id, ai_run_id, exposure_possible_at)
    SELECT d.project_id, d.report_id, d.stage, 'reviewer_opinion_reveal', d.id,
           d.proposal_id, d.ai_run_id, clock_timestamp()
    FROM ai_reviewer_decisions d
    LEFT JOIN screening_state s ON s.project_id=d.project_id AND s.report_id=d.report_id
    WHERE d.project_id=project AND (report IS NULL OR d.report_id=report)
      AND (d.resolved_at IS NOT NULL OR
           COALESCE(CASE WHEN d.stage='full_text' THEN s.full_text_status
                         ELSE s.title_abstract_status END, 'unscreened')
              NOT IN ('unscreened', 'not_required'))
    ON CONFLICT DO NOTHING
$$;

CREATE FUNCTION track_ai_opinion_availability() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='ai_proposals' THEN
        IF NEW.operation='screening_suggestion' THEN
            PERFORM record_available_ai_opinions(NEW.project_id, NEW.target_report_id);
        END IF;
    ELSIF TG_TABLE_NAME='project_ai_autonomy' THEN
        IF NEW.task IN ('title_abstract_screening','full_text_screening') THEN
            PERFORM record_available_ai_opinions(NEW.project_id, NULL);
        END IF;
    ELSE
        PERFORM record_available_ai_opinions(NEW.project_id, NEW.report_id);
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER ai_proposal_availability AFTER INSERT ON ai_proposals
    FOR EACH ROW EXECUTE FUNCTION track_ai_opinion_availability();
CREATE TRIGGER reviewer_opinion_availability AFTER INSERT OR UPDATE ON ai_reviewer_decisions
    FOR EACH ROW EXECUTE FUNCTION track_ai_opinion_availability();
CREATE TRIGGER screening_opinion_availability AFTER INSERT OR UPDATE ON screening_state
    FOR EACH ROW EXECUTE FUNCTION track_ai_opinion_availability();
CREATE TRIGGER autonomy_opinion_availability AFTER INSERT OR UPDATE ON project_ai_autonomy
    FOR EACH ROW EXECUTE FUNCTION track_ai_opinion_availability();
