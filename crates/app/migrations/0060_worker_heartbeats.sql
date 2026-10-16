-- Worker liveness for /health/dependencies.
--
-- Each worker process upserts its row on a timer (and deletes it on a clean
-- shutdown), so "is any worker alive" is a recent `seen_at`, not an inference
-- from queue age. Rows of crashed workers age out and are pruned by the
-- remaining workers.
CREATE TABLE worker_heartbeats (
  owner text PRIMARY KEY CHECK (length(owner) BETWEEN 1 AND 200),
  started_at timestamptz NOT NULL DEFAULT now(),
  seen_at timestamptz NOT NULL DEFAULT now()
);
