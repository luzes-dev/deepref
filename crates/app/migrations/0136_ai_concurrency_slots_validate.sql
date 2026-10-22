-- phase: expand
-- Validate the AI concurrency slot shapes that 0135 installed without blocking
-- writes. The table ships empty in the same release, so validation is a scan of
-- nothing in practice and keeps the expand/validate split intact.
ALTER TABLE ai_concurrency_slots VALIDATE CONSTRAINT ai_concurrency_slots_scope_kind_check;
ALTER TABLE ai_concurrency_slots VALIDATE CONSTRAINT ai_concurrency_slots_slot_index_check;
