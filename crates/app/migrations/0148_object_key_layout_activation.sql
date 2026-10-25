-- phase: expand
-- Default N writes keys N-1 can parse. Only the explicit drain-guarded
-- activation command may switch factories and retire the restrictive CHECK.
CREATE TABLE object_storage_rollout (
  singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
  tenant_keys_activated boolean NOT NULL DEFAULT false,
  activated_at timestamptz,
  parser_rollback_floor text,
  CHECK ((NOT tenant_keys_activated AND activated_at IS NULL AND parser_rollback_floor IS NULL)
    OR (tenant_keys_activated AND activated_at IS NOT NULL
        AND parser_rollback_floor IS NOT NULL AND parser_rollback_floor = 'tenant-object-keys-v1'))
);
INSERT INTO object_storage_rollout(singleton) VALUES(true);
