-- phase: expand
-- AI concurrency admission: the PostgreSQL leased-slot table that exists
-- before T-VALKEY (06) and remains the fallback whenever Valkey is absent (09).
-- Slots are operational capacity, never financial state: they never change a
-- reservation, a settlement or a ledger number. Advisory locks are deliberately
-- not used: they cannot be counted, expired or capacity-bounded, so they do not
-- implement leased concurrency.
--
-- One row is one slot of one scope key. A grant may only claim a row whose
-- lease is absent or expired and whose slot_index is below the scope's current
-- limit, so at most `limit` live leases can exist per key under any
-- interleaving. holder_id names the owning process; generation is drawn from a
-- sequence on every grant and is never reused, so a lease whose row was freed
-- (or swept) can always be told apart from a newer grant of the same slot.

CREATE TABLE IF NOT EXISTS ai_concurrency_slots (
  scope_kind text NOT NULL,
  scope_key text NOT NULL,
  slot_index integer NOT NULL,
  holder_id uuid NOT NULL,
  generation bigint NOT NULL,
  acquired_at timestamptz NOT NULL,
  lease_expires_at timestamptz NOT NULL,
  PRIMARY KEY (scope_kind, scope_key, slot_index)
);

ALTER TABLE ai_concurrency_slots
  ADD CONSTRAINT ai_concurrency_slots_scope_kind_check
  CHECK (scope_kind IN ('workspace', 'route')) NOT VALID;

ALTER TABLE ai_concurrency_slots
  ADD CONSTRAINT ai_concurrency_slots_slot_index_check
  CHECK (slot_index >= 0) NOT VALID;

CREATE SEQUENCE IF NOT EXISTS ai_concurrency_slots_generation_seq AS bigint;
