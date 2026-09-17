-- Fictional research data for local UI exploration. Safe to rerun; existing decisions are retained.
BEGIN;
INSERT INTO projects (id, name, description, default_max_depth)
VALUES ('00000000-0000-4000-8000-000000000201', 'Ambient AI in clinical practice',
'Sample review · How does ambient documentation affect clinician workload and quality of care?', 2)
ON CONFLICT (id) DO NOTHING;

CREATE TEMP TABLE ui_reports (ordinal integer, title text, year integer, citations integer) ON COMMIT DROP;
INSERT INTO ui_reports VALUES
(1, 'Ambient documentation and time spent in the electronic health record', 2025, 37),
(2, 'Clinician experience with AI-assisted medical notes: a multicentre trial', 2024, 83),
(3, 'Patient perspectives on ambient listening in primary care', 2025, 19),
(4, 'Accuracy of automated clinical documentation in outpatient visits', 2024, 62),
(5, 'Reducing after-hours documentation with an ambient scribe', 2023, 116),
(6, 'Six-month follow-up of an ambient documentation programme', 2025, 14),
(7, 'Documentation burden and physician wellbeing in community clinics', 2023, 91),
(8, 'Safety and omissions in AI-generated encounter summaries', 2025, 28),
(9, 'Ambient scribing in emergency care: a feasibility study', 2024, 43),
(10, 'Cost and implementation of clinical speech recognition', 2022, 154),
(11, 'Shared decision-making when an AI scribe joins the consultation', 2025, 11),
(12, 'Nursing workflow after introducing automated progress notes', 2024, 32);

INSERT INTO reports (id, title, abstract_text, publication_year, journal, container_title, total_citations, work_type, raw)
SELECT md5('deepref:ui:report:' || ordinal)::uuid, title,
'This is fictional sample evidence for exploring DeepRef. Background: Clinical documentation takes time away from patient care. Methods: We compared usual documentation with ambient assistance across outpatient teams. Outcomes included documentation time, note quality, and clinician experience. Results: Teams reported less after-hours documentation, with variation across specialties. Manual review remained necessary to identify omissions. Conclusions: Ambient assistance may reduce workload when paired with clinician review; longer follow-up is needed.',
year, 'Sample Journal of Clinical Informatics', 'Sample Journal of Clinical Informatics', citations, 'journal-article', '{"sample":true}'
FROM ui_reports ON CONFLICT (id) DO NOTHING;
INSERT INTO report_identifiers (id, report_id, scheme, value, normalized_value)
SELECT md5('deepref:ui:doi:' || ordinal)::uuid, md5('deepref:ui:report:' || ordinal)::uuid,
'doi', '10.5555/deepref.demo.' || ordinal, '10.5555/deepref.demo.' || ordinal
FROM ui_reports ON CONFLICT DO NOTHING;
INSERT INTO records (id, project_id, report_id, source, source_key, title, publication_year, raw)
SELECT md5('deepref:ui:record:' || ordinal)::uuid, '00000000-0000-4000-8000-000000000201',
md5('deepref:ui:report:' || ordinal)::uuid, 'sample', ordinal::text, title, year, '{"sample":true}'
FROM ui_reports ON CONFLICT DO NOTHING;
INSERT INTO project_reports (project_id, report_id, first_seen_record_id)
SELECT '00000000-0000-4000-8000-000000000201', md5('deepref:ui:report:' || ordinal)::uuid,
md5('deepref:ui:record:' || ordinal)::uuid FROM ui_reports ON CONFLICT DO NOTHING;
INSERT INTO citations (project_id, source_report_id, target_report_id, source, legacy_source_doi, legacy_target_doi)
SELECT '00000000-0000-4000-8000-000000000201', md5('deepref:ui:report:' || a.ordinal)::uuid,
md5('deepref:ui:report:' || b.ordinal)::uuid, 'sample', '10.5555/deepref.demo.' || a.ordinal,
'10.5555/deepref.demo.' || b.ordinal
FROM ui_reports a JOIN ui_reports b ON a.ordinal < b.ordinal AND (a.ordinal + b.ordinal) % 3 = 0
ON CONFLICT DO NOTHING;
SELECT recompute_project_report_metrics('00000000-0000-4000-8000-000000000201');
INSERT INTO protocol_versions (id, project_id, version, name, status, criteria, objective, question, framework_kind, framework_fields)
SELECT '00000000-0000-4000-8000-000000000202', '00000000-0000-4000-8000-000000000201', 1,
'Ambient documentation review', 'draft', '[]',
'Assess the effect of ambient AI documentation on clinical workload and quality.',
'Does ambient AI documentation reduce clinician workload without compromising note quality?',
'pico', '{"population":"Clinicians providing direct patient care","intervention":"Ambient AI documentation","comparator":"Usual documentation","outcome":"Documentation time, note quality, and clinician wellbeing"}'
WHERE NOT EXISTS (SELECT 1 FROM protocol_versions WHERE id = '00000000-0000-4000-8000-000000000202');
INSERT INTO eligibility_criteria (id, protocol_version_id, criterion_type, label, description, ordinal, stage, dimension)
SELECT md5('deepref:ui:criterion:' || c.ordinal)::uuid, p.id, 'include', c.label, c.description, c.ordinal, 'both', c.dimension
FROM protocol_versions p CROSS JOIN (VALUES
(0, 'Clinical care', 'Clinicians providing direct patient care.', 'population'),
(1, 'Ambient documentation', 'Ambient AI used during clinical encounters.', 'intervention'),
(2, 'Relevant outcomes', 'Documentation time, note quality, or clinician wellbeing.', 'outcome')
) AS c(ordinal, label, description, dimension)
WHERE p.id = '00000000-0000-4000-8000-000000000202' AND p.status = 'draft'
ON CONFLICT DO NOTHING;
UPDATE protocol_versions SET status = 'published', published_at = now()
WHERE id = '00000000-0000-4000-8000-000000000202' AND status = 'draft';
INSERT INTO screening_state (project_id, report_id, title_abstract_status, full_text_status, final_status)
SELECT '00000000-0000-4000-8000-000000000201', md5('deepref:ui:report:' || ordinal)::uuid,
CASE WHEN ordinal <= 4 THEN 'include' WHEN ordinal = 10 THEN 'exclude' WHEN ordinal = 12 THEN 'maybe' ELSE 'unscreened' END,
CASE WHEN ordinal <= 2 THEN 'include' WHEN ordinal <= 4 THEN 'unscreened' ELSE 'not_required' END,
CASE WHEN ordinal <= 2 THEN 'include' WHEN ordinal <= 4 THEN 'pending_full_text' WHEN ordinal = 10 THEN 'exclude' WHEN ordinal = 12 THEN 'maybe' ELSE 'unscreened' END
FROM ui_reports ON CONFLICT DO NOTHING;
INSERT INTO studies (id, project_id, title)
VALUES ('00000000-0000-4000-8000-000000000203', '00000000-0000-4000-8000-000000000201', 'Ambient documentation in primary care'),
('00000000-0000-4000-8000-000000000204', '00000000-0000-4000-8000-000000000201', 'Multicentre clinician experience trial')
ON CONFLICT DO NOTHING;
INSERT INTO study_reports (project_id, study_id, report_id)
VALUES ('00000000-0000-4000-8000-000000000201', '00000000-0000-4000-8000-000000000203', md5('deepref:ui:report:1')::uuid),
('00000000-0000-4000-8000-000000000201', '00000000-0000-4000-8000-000000000203', md5('deepref:ui:report:6')::uuid),
('00000000-0000-4000-8000-000000000201', '00000000-0000-4000-8000-000000000204', md5('deepref:ui:report:2')::uuid)
ON CONFLICT DO NOTHING;
INSERT INTO extraction_field_definitions (id, project_id, version, field_key, label, value_type, required)
VALUES ('00000000-0000-4000-8000-000000000205', '00000000-0000-4000-8000-000000000201', 1, 'sample_size', 'Sample size', 'number', true),
('00000000-0000-4000-8000-000000000206', '00000000-0000-4000-8000-000000000201', 1, 'primary_outcome', 'Primary outcome', 'text', true)
ON CONFLICT DO NOTHING;
COMMIT;
