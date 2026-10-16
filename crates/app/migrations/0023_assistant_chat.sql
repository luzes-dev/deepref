-- Migration 0023: Assistant conversations and messages persistence

CREATE TABLE assistant_conversations (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  title text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 500),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX assistant_conversations_project_updated_idx
  ON assistant_conversations (project_id, updated_at DESC);

CREATE TABLE assistant_messages (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  conversation_id uuid NOT NULL REFERENCES assistant_conversations(id) ON DELETE CASCADE,
  role text NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
  content text NOT NULL,
  tool_calls jsonb CHECK (tool_calls IS NULL OR jsonb_typeof(tool_calls) = 'array'),
  tool_results jsonb CHECK (tool_results IS NULL OR jsonb_typeof(tool_results) IN ('array', 'object')),
  metadata jsonb CHECK (metadata IS NULL OR jsonb_typeof(metadata) = 'object'),
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX assistant_messages_conversation_created_idx
  ON assistant_messages (conversation_id, created_at ASC);
