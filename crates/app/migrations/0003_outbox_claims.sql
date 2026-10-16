ALTER TABLE event_outbox
  ADD COLUMN IF NOT EXISTS locked_at timestamptz,
  ADD COLUMN IF NOT EXISTS attempts integer NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS last_error text;

CREATE INDEX IF NOT EXISTS event_outbox_unpublished_idx
  ON event_outbox (created_at)
  WHERE published_at IS NULL;
