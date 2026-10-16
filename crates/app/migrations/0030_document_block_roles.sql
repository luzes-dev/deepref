-- The structure-aware parser stores the block role in `kind` (see
-- deepref_documents::structure::Role). Allow the roles it emits, keeping the
-- legacy values so older parser versions stay valid.
ALTER TABLE document_blocks DROP CONSTRAINT IF EXISTS document_blocks_kind_check;
ALTER TABLE document_blocks
  ADD CONSTRAINT document_blocks_kind_check CHECK
    (kind IN ('text', 'heading', 'table', 'figure_caption', 'caption', 'reference', 'title', 'front_matter'));
