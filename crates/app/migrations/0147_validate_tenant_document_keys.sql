-- phase: expand
-- Validate the expanded key grammar separately from the short ALTER lock.
ALTER TABLE documents VALIDATE CONSTRAINT documents_object_key_tenant_layout_check;
