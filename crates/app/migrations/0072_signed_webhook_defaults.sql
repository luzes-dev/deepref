-- Keep the database default aligned with explicit new-workflow creation.
-- Existing unsigned endpoints remain an explicit owner choice.
ALTER TABLE workflows ALTER COLUMN webhook_signature_required SET DEFAULT true;
