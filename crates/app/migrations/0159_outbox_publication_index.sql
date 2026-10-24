-- no-transaction
-- phase: expand
CREATE INDEX CONCURRENTLY IF NOT EXISTS outbox_publication_due_idx
  ON outbox_events(publication_next_at,created_at,event_id)
  WHERE published_at IS NULL AND delivery_state='pending';
