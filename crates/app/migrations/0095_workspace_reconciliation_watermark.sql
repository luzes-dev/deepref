-- phase: expand
-- Ordering evidence for WorkOS reconciliation (handoff 03a).
--
-- A reconciliation snapshot is only as fresh as the moment its fetch began.
-- `workos_reconciled_through` is the fetch-start time of the newest snapshot
-- applied to the workspace; a snapshot that began earlier is stale and is
-- rejected whole, so a delayed pre-removal snapshot can never restore revoked
-- access.
ALTER TABLE workspaces ADD COLUMN workos_reconciled_through timestamptz;
