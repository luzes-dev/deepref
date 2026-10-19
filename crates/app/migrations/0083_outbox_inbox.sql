-- Broker-independent publication outbox and provider integration inbox
-- (handoff 04a). PostgreSQL stays authoritative: an outbox row is inserted in
-- the same transaction as its authoritative change and is `delivered` only
-- when every expected consumer's receipt has committed. Claiming or leasing a
-- row never marks it delivered. This is the row contract the later JetStream
-- relay (04b) reuses: broker publication is recorded in `published_at`
-- separately from handler delivery. The domain-specific `ai_autonomy_outbox`
-- is deliberately not consolidated into these tables.

CREATE TABLE outbox_events (
  event_id uuid PRIMARY KEY,
  message_type text NOT NULL,
  message_version integer NOT NULL CHECK (message_version > 0),
  -- Stable identifiers and routing metadata, not a second copy of
  -- authoritative aggregate state.
  payload jsonb NOT NULL,
  -- Frozen at insert from the message contract (type + version), so what
  -- `delivered` means never silently changes for rows already committed.
  expected_consumers text[] NOT NULL CHECK (cardinality(expected_consumers) > 0),
  created_at timestamptz NOT NULL DEFAULT now(),
  delivery_state text NOT NULL DEFAULT 'pending'
    CHECK (delivery_state IN ('pending', 'delivered', 'quarantined')),
  -- Set by the 04b JetStream relay on broker acceptance; NULL in 04a. Publish
  -- confirmation is not delivery and never completes a row.
  published_at timestamptz,
  delivered_at timestamptz,
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts integer NOT NULL DEFAULT 8 CHECK (max_attempts > 0),
  next_attempt_at timestamptz NOT NULL DEFAULT now(),
  last_error text,
  lease_owner text,
  lease_expires_at timestamptz,
  -- Incremented on each claim. The monotonically increasing generation fences
  -- a stale worker even when a reclaimed row has the same lease_owner.
  claim_generation bigint NOT NULL DEFAULT 0 CHECK (claim_generation >= 0),
  CHECK ((lease_owner IS NULL) = (lease_expires_at IS NULL)),
  CHECK ((delivery_state = 'delivered') = (delivered_at IS NOT NULL))
);

-- Claimable rows: pending, due and without a live lease.
CREATE INDEX outbox_events_claim_idx
  ON outbox_events (next_attempt_at, created_at)
  WHERE delivery_state = 'pending';
CREATE INDEX outbox_events_created_idx ON outbox_events (created_at);
-- The ordinary publication loop (04b) selects rows the relay has not yet
-- published.
CREATE INDEX outbox_events_unpublished_idx
  ON outbox_events (created_at)
  WHERE published_at IS NULL;

-- Delivery is modeled per consumer: one row per (event, consumer) carrying
-- that recipient's retry state. A consumer that already holds a delivered
-- receipt treats redelivery as a no-op. Rows for consumers outside the event's
-- frozen expected set are kept as evidence but never counted toward
-- completion.
CREATE TABLE outbox_receipts (
  event_id uuid NOT NULL REFERENCES outbox_events (event_id) ON DELETE CASCADE,
  consumer text NOT NULL,
  state text NOT NULL
    CHECK (state IN ('retrying', 'delivered', 'quarantined')),
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  next_attempt_at timestamptz NOT NULL DEFAULT now(),
  last_error text,
  delivered_at timestamptz,
  PRIMARY KEY (event_id, consumer),
  CHECK ((state = 'delivered') = (delivered_at IS NOT NULL))
);

CREATE INDEX outbox_receipts_pending_idx
  ON outbox_receipts (event_id, next_attempt_at)
  WHERE state = 'retrying';

-- Inbound provider webhooks (WorkOS, Polar, later). The provider-scoped
-- deduplication key makes a duplicate delivery a committed no-op. Signatures
-- are verified before any insert; a rejected request creates no row. Receive
-- acknowledges with 2xx only after the insert (or duplicate check) commits.
CREATE TABLE integration_inbox_events (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  provider text NOT NULL,
  provider_event_id text NOT NULL
    CHECK (char_length(provider_event_id) BETWEEN 1 AND 255),
  event_type text,
  payload jsonb NOT NULL,
  received_at timestamptz NOT NULL DEFAULT now(),
  processing_state text NOT NULL DEFAULT 'received'
    CHECK (processing_state IN ('received', 'processing', 'processed', 'failed', 'quarantined')),
  attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts integer NOT NULL DEFAULT 8 CHECK (max_attempts > 0),
  next_attempt_at timestamptz NOT NULL DEFAULT now(),
  last_error text,
  processed_at timestamptz,
  quarantine_reason text,
  lease_owner text,
  lease_expires_at timestamptz,
  claim_generation bigint NOT NULL DEFAULT 0 CHECK (claim_generation >= 0),
  CHECK ((lease_owner IS NULL) = (lease_expires_at IS NULL)),
  UNIQUE (provider, provider_event_id),
  CHECK ((processing_state = 'processed') = (processed_at IS NOT NULL))
);

CREATE INDEX integration_inbox_claim_idx
  ON integration_inbox_events (next_attempt_at, received_at)
  WHERE processing_state IN ('received', 'failed', 'processing');
CREATE INDEX integration_inbox_quarantine_idx
  ON integration_inbox_events (received_at)
  WHERE processing_state = 'quarantined';
