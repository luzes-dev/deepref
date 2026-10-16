CREATE SEQUENCE IF NOT EXISTS graph_domain_revision_seq AS bigint;

CREATE TABLE IF NOT EXISTS domain_events (
  event_id uuid PRIMARY KEY,
  schema_version smallint NOT NULL CHECK (schema_version > 0),
  event_type text NOT NULL,
  entity_type text NOT NULL,
  entity_key text NOT NULL,
  revision bigint NOT NULL UNIQUE,
  payload jsonb NOT NULL,
  correlation_id uuid NOT NULL,
  causation_id uuid,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS domain_events_entity_revision_idx
  ON domain_events (entity_type, entity_key, revision);
CREATE INDEX IF NOT EXISTS domain_events_created_idx ON domain_events (created_at, revision);

CREATE TABLE IF NOT EXISTS domain_tombstones (
  entity_type text NOT NULL,
  entity_key text NOT NULL,
  project_id uuid,
  revision bigint NOT NULL,
  event_id uuid NOT NULL REFERENCES domain_events(event_id),
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (entity_type, entity_key, revision)
);

CREATE TABLE IF NOT EXISTS projection_state (
  projection_name text NOT NULL,
  project_id uuid,
  state text NOT NULL DEFAULT 'pending',
  revision bigint NOT NULL DEFAULT 0,
  watermark bigint NOT NULL DEFAULT 0,
  lag bigint NOT NULL DEFAULT 0 CHECK (lag >= 0),
  last_success_at timestamptz,
  last_error text,
  rebuild_state text,
  updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS projection_state_identity_idx
  ON projection_state (projection_name, COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid));

CREATE TABLE IF NOT EXISTS metric_snapshots (
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  revision bigint NOT NULL,
  metrics_as_of timestamptz NOT NULL,
  work_count bigint NOT NULL,
  edge_count bigint NOT NULL,
  payload jsonb NOT NULL DEFAULT '{}'::jsonb,
  PRIMARY KEY (project_id, revision)
);
CREATE INDEX IF NOT EXISTS metric_snapshots_latest_idx
  ON metric_snapshots (project_id, metrics_as_of DESC);

INSERT INTO projection_state (projection_name, project_id, state)
VALUES ('graph', NULL, 'pending') ON CONFLICT DO NOTHING;
