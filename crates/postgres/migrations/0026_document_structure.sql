-- Document structure produced by the parse step (native parser and optional
-- GROBID enrichment). Rows belong to a (document, parser_version) pair; the
-- active set is the one matching documents.active_parser_version.
CREATE TABLE IF NOT EXISTS document_references (
  id uuid PRIMARY KEY,
  document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  parser_version text NOT NULL,
  ordinal integer NOT NULL CHECK (ordinal > 0),
  raw text NOT NULL,
  title text,
  authors jsonb NOT NULL DEFAULT '[]'::jsonb,
  year integer,
  venue text,
  doi text,
  source text NOT NULL CHECK (source IN ('native', 'grobid')),
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (document_id, parser_version, ordinal)
);
CREATE INDEX IF NOT EXISTS document_references_doi_idx
  ON document_references (lower(doi)) WHERE doi IS NOT NULL;

CREATE TABLE IF NOT EXISTS document_sections (
  id uuid PRIMARY KEY,
  document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  parser_version text NOT NULL,
  ordinal integer NOT NULL CHECK (ordinal > 0),
  number text,
  title text NOT NULL,
  depth integer NOT NULL CHECK (depth > 0),
  path text[] NOT NULL DEFAULT '{}',
  source text NOT NULL CHECK (source IN ('native', 'grobid')),
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (document_id, parser_version, ordinal)
);

-- document_blocks.section_path already exists (0006); the persisted value is now
-- filled from the parser/GROBID instead of always being empty.
