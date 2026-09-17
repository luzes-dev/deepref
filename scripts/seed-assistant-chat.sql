-- Fictional assistant conversations for local UI exploration of /projects/{id}/assistant.
-- Safe to rerun: fixed UUIDs, existing rows are retained. Requires the demo project
-- from seed-ui.sql ('00000000-0000-4000-8000-000000000201') and migration 0023.
BEGIN;

-- Convenience handles for the fictional seed-ui records referenced below.
CREATE TEMP TABLE seed_assistant ON COMMIT DROP AS
SELECT '00000000-0000-4000-8000-000000000201'::uuid AS project_id,
       '00000000-0000-4000-8000-000000000202'::uuid AS protocol_id,
       md5('deepref:ui:report:1')::uuid AS report_1,
       md5('deepref:ui:report:2')::uuid AS report_2,
       md5('deepref:ui:report:3')::uuid AS report_3,
       md5('deepref:ui:report:4')::uuid AS report_4,
       md5('deepref:ui:report:5')::uuid AS report_5,
       md5('deepref:ui:record:2')::uuid AS record_2;

INSERT INTO assistant_conversations (id, project_id, title, created_at, updated_at)
SELECT v.id, s.project_id, v.title, v.created_at, v.updated_at
FROM (VALUES
  ('a0000000-0000-4000-8000-000000000001'::uuid, 'Protocol & inclusion criteria',
   '2026-09-10T09:05:00+00'::timestamptz, '2026-09-10T09:12:00+00'::timestamptz, 1),
  ('a0000000-0000-4000-8000-000000000002'::uuid, 'Duplicate checks for scribe studies',
   '2026-09-10T10:20:00+00'::timestamptz, '2026-09-11T10:30:00+00'::timestamptz, 2),
  ('a0000000-0000-4000-8000-000000000003'::uuid, 'Full-text feasibility scan',
   '2026-09-10T13:50:00+00'::timestamptz, '2026-09-10T14:10:00+00'::timestamptz, 3),
  ('a0000000-0000-4000-8000-000000000004'::uuid, 'Screening queue prep',
   '2026-09-11T16:20:00+00'::timestamptz, '2026-09-11T16:40:00+00'::timestamptz, 4),
  ('a0000000-0000-4000-8000-000000000005'::uuid, 'Welcome tour',
   '2026-09-09T08:30:00+00'::timestamptz, '2026-09-09T08:31:00+00'::timestamptz, 5)
) AS v(id, title, created_at, updated_at, ordinal)
CROSS JOIN seed_assistant s
WHERE EXISTS (SELECT 1 FROM projects p WHERE p.id = s.project_id)
ON CONFLICT (id) DO NOTHING;

INSERT INTO assistant_messages (id, conversation_id, role, content, tool_calls, tool_results, metadata, created_at)
SELECT v.id, c.id, v.role, v.content, v.tool_calls::jsonb, v.tool_results::jsonb, v.metadata::jsonb, v.created_at
FROM (VALUES

  -- ------------------------------------------------------------------
  -- Conversation 1 · Protocol & inclusion criteria (read tool + citations)
  -- ------------------------------------------------------------------
  ('b0000000-0000-4000-8000-000000000101'::uuid,
   'a0000000-0000-4000-8000-000000000001'::uuid, 'user',
   'What does our protocol say about the target population and outcomes? I want to sanity-check the criteria before I start the title-abstract pass.',
   NULL, NULL,
   NULL, '2026-09-10T09:06:00+00'::timestamptz),

  ('b0000000-0000-4000-8000-000000000102'::uuid,
   'a0000000-0000-4000-8000-000000000001'::uuid, 'assistant',
   E'Here is the published protocol (v1, "Ambient documentation review"):\n\n• Population — clinicians providing direct patient care.\n• Intervention — ambient AI documentation used during clinical encounters.\n• Comparator — usual documentation.\n• Outcomes — documentation time, note quality, and clinician wellbeing.\n\nFor the title-abstract pass, that means reports are eligible when the setting is direct clinical care and the documentation support runs ambiently during the encounter. Retrospective dictation tools alone are borderline; flag them rather than excluding outright, and let the full-text stage settle it.',
   '[{"id":"call-proto-1","tool":"get_project_protocol","args":{"project_id":"00000000-0000-4000-8000-000000000201"}}]'::text,
   '[{"tool_call_id":"call-proto-1","tool":"get_project_protocol","output":{"id":"00000000-0000-4000-8000-000000000202","project_id":"00000000-0000-4000-8000-000000000201","version":1,"name":"Ambient documentation review","status":"published","objective":"Assess the effect of ambient AI documentation on clinical workload and quality.","question":"Does ambient AI documentation reduce clinician workload without compromising note quality?","framework":{"kind":"pico","population":"Clinicians providing direct patient care","intervention":"Ambient AI documentation","comparator":"Usual documentation","outcome":"Documentation time, note quality, and clinician wellbeing"},"criteria":[{"label":"Clinical care","description":"Clinicians providing direct patient care.","stage":"both"},{"label":"Ambient documentation","description":"Ambient AI used during clinical encounters.","stage":"both"},{"label":"Relevant outcomes","description":"Documentation time, note quality, or clinician wellbeing.","stage":"both"}],"revision":0,"published_at":"2026-09-01T09:00:00Z"},"proposal_review_run_id":null}]'::text,
   '{"message_id":"f0000000-0000-4000-8000-0000000000a1","system_invariants":"scientific_truth_requires_human_sign_off","input_tokens":46,"output_tokens":214}'::text,
   '2026-09-10T09:07:00+00'::timestamptz),

  -- ------------------------------------------------------------------
  -- Conversation 2 · Duplicate checks (search + proposal + citations)
  -- ------------------------------------------------------------------
  ('b0000000-0000-4000-8000-000000000201'::uuid,
   'a0000000-0000-4000-8000-000000000002'::uuid, 'user',
   'I suspect the two multicentre scribe trials are the same study. Can you check for duplicates and queue anything you find?',
   NULL, NULL,
   NULL, '2026-09-11T10:21:00+00'::timestamptz),

  ('b0000000-0000-4000-8000-000000000202'::uuid,
   'a0000000-0000-4000-8000-000000000002'::uuid, 'assistant',
   E'I searched the project reports for multicentre scribe trials and found three close matches, two of which share population, intervention, and follow-up window.\n\nI queued a duplicate merge proposal (review run d0000000…d1). Nothing has been merged — the records stay untouched until you approve or reject the proposal in the deduplication queue.',
   '[{"id":"call-search-1","tool":"search_project_reports","args":{"project_id":"00000000-0000-4000-8000-000000000201","query":"AI-assisted medical notes multicentre trial","limit":5}},{"id":"call-dup-1","tool":"propose_duplicate_merge","args":{"project_id":"00000000-0000-4000-8000-000000000201","source_record_id":"__RECORD_2__","candidate_report_id":"__REPORT_4__"}}]'::text,
   '[{"tool_call_id":"call-search-1","tool":"search_project_reports","output":[{"id":"__REPORT_2__","project_id":"00000000-0000-4000-8000-000000000201","title":"Clinician experience with AI-assisted medical notes: a multicentre trial","publication_year":2024,"journal":"Sample Journal of Clinical Informatics"},{"id":"__REPORT_4__","project_id":"00000000-0000-4000-8000-000000000201","title":"Accuracy of automated clinical documentation in outpatient visits","publication_year":2024,"journal":"Sample Journal of Clinical Informatics"},{"id":"__REPORT_5__","project_id":"00000000-0000-4000-8000-000000000201","title":"Reducing after-hours documentation with an ambient scribe","publication_year":2023,"journal":"Sample Journal of Clinical Informatics"}]},{"tool_call_id":"call-dup-1","tool":"propose_duplicate_merge","output":{"review_run_id":"d0000000-0000-4000-8000-0000000000d1","status_path":"/projects/00000000-0000-4000-8000-000000000201/review-runs/d0000000-0000-4000-8000-0000000000d1"},"proposal_review_run_id":"d0000000-0000-4000-8000-0000000000d1"}]'::text,
   '{"message_id":"f0000000-0000-4000-8000-0000000000a2","system_invariants":"scientific_truth_requires_human_sign_off","input_tokens":38,"output_tokens":186}'::text,
   '2026-09-11T10:22:00+00'::timestamptz),

  -- ------------------------------------------------------------------
  -- Conversation 3 · Full-text feasibility scan (multi-tool, one FAILED)
  -- ------------------------------------------------------------------
  ('b0000000-0000-4000-8000-000000000301'::uuid,
   'a0000000-0000-4000-8000-000000000003'::uuid, 'user',
   'Pull the exclusion-reason reporting from the emergency-care feasibility paper, and also check the latest RoB 2 appraisal for the outpatient accuracy study.',
   NULL, NULL,
   NULL, '2026-09-10T13:51:00+00'::timestamptz),

  ('b0000000-0000-4000-8000-000000000302'::uuid,
   'a0000000-0000-4000-8000-000000000003'::uuid, 'assistant',
   E'The document search surfaced two relevant blocks and I verified both by reading them directly:\n\n• Methods (page 4) — "Forty-two encounters were approached; six were excluded (three audio consent refusals, two technical failures, one abandoned encounter)." All six exclusions are documented with reasons.\n• Limitations (page 7) — the authors acknowledge that exclusion reasons were self-reported by the scribe operator.\n\nThe RoB 2 lookup failed: no completed appraisal version exists yet for that report, so there is nothing to read. Once a reviewer completes one, I can pull it here.',
   '[{"id":"call-docsearch-1","tool":"search_document","args":{"project_id":"00000000-0000-4000-8000-000000000201","document_id":"c0000000-0000-4000-8000-00000000c001","query":"exclusion reasons","limit":10}},{"id":"call-read-1","tool":"read_document_blocks","args":{"project_id":"00000000-0000-4000-8000-000000000201","document_id":"c0000000-0000-4000-8000-00000000c001","block_ids":["c0000000-0000-4000-8000-00000000c0b1","c0000000-0000-4000-8000-00000000c0b2"]}},{"id":"call-appraisal-1","tool":"get_appraisal","args":{"project_id":"00000000-0000-4000-8000-000000000201","report_id":"__REPORT_4__","definition_id":"rob2","definition_version":1}}]'::text,
   '[{"tool_call_id":"call-docsearch-1","tool":"search_document","output":[{"id":"c0000000-0000-4000-8000-00000000c0b1","document_id":"c0000000-0000-4000-8000-00000000c001","page_number":4,"kind":"paragraph","section_path":"Methods","ordinal":12,"text":"Forty-two encounters were approached; six were excluded (three audio consent refusals, two technical failures, one abandoned encounter). All exclusions were logged with operator reasons."},{"id":"c0000000-0000-4000-8000-00000000c0b2","document_id":"c0000000-0000-4000-8000-00000000c001","page_number":7,"kind":"paragraph","section_path":"Limitations","ordinal":31,"text":"Exclusion reasons were self-reported by the scribe operator and were not independently audited."}]},{"tool_call_id":"call-read-1","tool":"read_document_blocks","output":[{"id":"c0000000-0000-4000-8000-00000000c0b1","document_id":"c0000000-0000-4000-8000-00000000c001","page_number":4,"kind":"paragraph","section_path":"Methods","ordinal":12,"text":"Forty-two encounters were approached; six were excluded (three audio consent refusals, two technical failures, one abandoned encounter). All exclusions were logged with operator reasons."},{"id":"c0000000-0000-4000-8000-00000000c0b2","document_id":"c0000000-0000-4000-8000-00000000c001","page_number":7,"kind":"paragraph","section_path":"Limitations","ordinal":31,"text":"Exclusion reasons were self-reported by the scribe operator and were not independently audited."}]}]'::text,
   '{"message_id":"f0000000-0000-4000-8000-0000000000a3","system_invariants":"scientific_truth_requires_human_sign_off","input_tokens":64,"output_tokens":242}'::text,
   '2026-09-10T13:53:00+00'::timestamptz),

  -- ------------------------------------------------------------------
  -- Conversation 4 · Screening queue prep (proposal + plain follow-up)
  -- ------------------------------------------------------------------
  ('b0000000-0000-4000-8000-000000000401'::uuid,
   'a0000000-0000-4000-8000-000000000004'::uuid, 'user',
   'Prepare a screening proposal for the patient-perspectives report at title-abstract stage.',
   NULL, NULL,
   NULL, '2026-09-11T16:21:00+00'::timestamptz),

  ('b0000000-0000-4000-8000-000000000402'::uuid,
   'a0000000-0000-4000-8000-000000000004'::uuid, 'assistant',
   E'Queued a title-abstract screening proposal for "Patient perspectives on ambient listening in primary care" (review run d0000000…d2).\n\nRationale in the proposal: the study covers clinicians'' and patients'' experiences of ambient documentation, which matches the population and intervention criteria, but the reported outcomes are experience-focused rather than the protocol''s time/quality/wellbeing triad — worth a careful look at full text.',
   '[{"id":"call-screen-1","tool":"propose_screening_decision","args":{"project_id":"00000000-0000-4000-8000-000000000201","report_id":"__REPORT_3__","stage":"title_abstract"}}]'::text,
   '[{"tool_call_id":"call-screen-1","tool":"propose_screening_decision","output":{"review_run_id":"d0000000-0000-4000-8000-0000000000d2","status_path":"/projects/00000000-0000-4000-8000-000000000201/review-runs/d0000000-0000-4000-8000-0000000000d2"},"proposal_review_run_id":"d0000000-0000-4000-8000-0000000000d2"}]'::text,
   '{"message_id":"f0000000-0000-4000-8000-0000000000a4","system_invariants":"scientific_truth_requires_human_sign_off","input_tokens":32,"output_tokens":158}'::text,
   '2026-09-11T16:22:00+00'::timestamptz),

  ('b0000000-0000-4000-8000-000000000403'::uuid,
   'a0000000-0000-4000-8000-000000000004'::uuid, 'user',
   'Got it. And what happens if I disagree with the rationale?',
   NULL, NULL,
   NULL, '2026-09-11T16:39:00+00'::timestamptz),

  ('b0000000-0000-4000-8000-000000000404'::uuid,
   'a0000000-0000-4000-8000-000000000004'::uuid, 'assistant',
   E'The proposal is only a suggestion — you stay in control:\n\n1. Open the screening queue and review the proposal side by side with the report abstract.\n2. Edit the decision or the rationale before you commit, or reject the proposal outright.\n3. Your decision is what gets recorded; the proposal itself never mutates a report.\n\nIf you disagree, rejecting it also tells the assistant to stop suggesting that direction for the report.',
   NULL, NULL,
   '{"message_id":"f0000000-0000-4000-8000-0000000000a5","system_invariants":"scientific_truth_requires_human_sign_off","input_tokens":21,"output_tokens":132}'::text,
   '2026-09-11T16:40:00+00'::timestamptz),

  -- ------------------------------------------------------------------
  -- Conversation 5 · Welcome tour (no tools, plain muted bubble)
  -- ------------------------------------------------------------------
  ('b0000000-0000-4000-8000-000000000501'::uuid,
   'a0000000-0000-4000-8000-000000000005'::uuid, 'user',
   'Hi! What can you help me with in this project?',
   NULL, NULL,
   NULL, '2026-09-09T08:30:30+00'::timestamptz),

  ('b0000000-0000-4000-8000-000000000502'::uuid,
   'a0000000-0000-4000-8000-000000000005'::uuid, 'assistant',
   E'Welcome! I am DeepRef''s assistant for the "Ambient AI in clinical practice" review. I can:\n\n• Read the published protocol and its eligibility criteria.\n• Search reports and full-text document blocks, then quote them back with citations.\n• Propose screening decisions, duplicate merges, study groupings, classifications, extractions, and appraisal answers — always into the human review queue.\n• Trigger background automation recipes for recurring work.\n\nI never change scientific records directly. Everything I propose waits for your sign-off.',
   NULL, NULL,
   '{"message_id":"f0000000-0000-4000-8000-0000000000a6","system_invariants":"scientific_truth_requires_human_sign_off","input_tokens":14,"output_tokens":118}'::text,
   '2026-09-09T08:31:00+00'::timestamptz)

) AS v(id, conversation_id, role, content, tool_calls, tool_results, metadata, created_at)
JOIN assistant_conversations c ON c.id = v.conversation_id
ON CONFLICT (id) DO NOTHING;

-- Substitute the fictional seed-ui identifiers into the JSON payloads.
UPDATE assistant_messages
SET tool_calls = replace(replace(tool_calls::text, '__RECORD_2__', (SELECT record_2::text FROM seed_assistant)), '__REPORT_4__', (SELECT report_4::text FROM seed_assistant))::jsonb,
    tool_results = replace(replace(replace(replace(tool_results::text,
        '__RECORD_2__', (SELECT record_2::text FROM seed_assistant)),
        '__REPORT_2__', (SELECT report_2::text FROM seed_assistant)),
        '__REPORT_3__', (SELECT report_3::text FROM seed_assistant)),
        '__REPORT_4__', (SELECT report_4::text FROM seed_assistant))::jsonb
WHERE id IN (
  'b0000000-0000-4000-8000-000000000202'::uuid,
  'b0000000-0000-4000-8000-000000000302'::uuid,
  'b0000000-0000-4000-8000-000000000402'::uuid
);

-- Keep conversation activity stamps in sync with the newest seeded message.
UPDATE assistant_conversations c
SET updated_at = m.latest
FROM (
  SELECT conversation_id, max(created_at) AS latest
  FROM assistant_messages
  WHERE conversation_id IN (
    'a0000000-0000-4000-8000-000000000001'::uuid,
    'a0000000-0000-4000-8000-000000000002'::uuid,
    'a0000000-0000-4000-8000-000000000003'::uuid,
    'a0000000-0000-4000-8000-000000000004'::uuid,
    'a0000000-0000-4000-8000-000000000005'::uuid
  )
  GROUP BY conversation_id
) m
WHERE c.id = m.conversation_id;

COMMIT;
