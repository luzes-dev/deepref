-- phase: expand
-- Diagnostic evidence only. NULL supports old heartbeat writers; heartbeat
-- freshness cannot renew adapter observation time or authorize a claim/drain.
ALTER TABLE worker_heartbeats
  ADD COLUMN transport_mode text CHECK (transport_mode IN ('postgres','jetstream')),
  ADD COLUMN transport_ready boolean,
  ADD COLUMN transport_observed_at timestamptz;
