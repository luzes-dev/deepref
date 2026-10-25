-- phase: expand
-- Publication has its own lease/retry budget: it never spends consumer attempts
-- and broker acceptance never completes a consumer receipt.
ALTER TABLE outbox_events
  ADD COLUMN publication_owner text,
  ADD COLUMN publication_until timestamptz,
  ADD COLUMN publication_generation bigint NOT NULL DEFAULT 0,
  ADD COLUMN publication_epoch bigint NOT NULL DEFAULT 0,
  ADD COLUMN publication_attempts integer NOT NULL DEFAULT 0,
  ADD COLUMN publication_next_at timestamptz NOT NULL DEFAULT now(),
  ADD COLUMN publication_error text,
  ADD CONSTRAINT outbox_publication_lease_pair CHECK
    ((publication_owner IS NULL) = (publication_until IS NULL));

-- Independent of application delivery and retained for operator diagnosis.
CREATE TABLE transport_dead_letters (
  delivery_id text PRIMARY KEY,
  event_id uuid REFERENCES outbox_events(event_id) ON DELETE CASCADE,
  consumer text NOT NULL,
  publication_epoch bigint,
  reason text NOT NULL,
  deliveries bigint NOT NULL,
  recorded_at timestamptz NOT NULL DEFAULT now()
);
