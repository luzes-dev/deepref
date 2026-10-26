-- phase: expand
-- Separate repair/attempt identity from successful graph output progress.
ALTER TABLE projects
  ADD COLUMN metric_projection_generation bigint NOT NULL DEFAULT 0 CHECK (metric_projection_generation >= 0),
  ADD COLUMN metric_projection_attempt bigint NOT NULL DEFAULT 0 CHECK (metric_projection_attempt >= 0);
ALTER TABLE projection_state
  ADD COLUMN generation bigint NOT NULL DEFAULT 0 CHECK (generation >= 0),
  ADD COLUMN attempt bigint NOT NULL DEFAULT 0 CHECK (attempt >= 0);
