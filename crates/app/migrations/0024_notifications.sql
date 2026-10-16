-- Migration 0024: User-facing notification inbox

CREATE SEQUENCE notification_revision_seq AS bigint;

CREATE TABLE notifications (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  revision bigint NOT NULL DEFAULT nextval('notification_revision_seq'),
  kind text NOT NULL CHECK (length(btrim(kind)) BETWEEN 1 AND 100),
  severity text NOT NULL CHECK (severity IN ('success', 'info', 'warning', 'error')),
  project_id uuid REFERENCES projects(id) ON DELETE CASCADE,
  title text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
  body text CHECK (body IS NULL OR length(btrim(body)) <= 500),
  payload jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(payload) = 'object'),
  read_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE notifications IS
  'Durable human-readable records of server-side async outcomes (imports, review runs, automations).';

CREATE INDEX notifications_recent_idx ON notifications (revision DESC);
CREATE INDEX notifications_unread_idx ON notifications (revision DESC) WHERE read_at IS NULL;
CREATE INDEX notifications_project_idx ON notifications (project_id, revision DESC);
