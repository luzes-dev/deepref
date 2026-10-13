-- PR10: keep PRISMA freshness tied to exported exclusion-reason definitions.
-- The append-only decision/study event tables remain the source for transition
-- timestamps; this column covers labels and codes rendered in the reason list.

ALTER TABLE exclusion_reasons
  ADD COLUMN IF NOT EXISTS updated_at timestamptz NOT NULL DEFAULT now();

CREATE OR REPLACE FUNCTION deepref_touch_exclusion_reason_updated_at()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  NEW.updated_at = now();
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS exclusion_reasons_touch_updated_at ON exclusion_reasons;
CREATE TRIGGER exclusion_reasons_touch_updated_at
BEFORE UPDATE ON exclusion_reasons
FOR EACH ROW
EXECUTE FUNCTION deepref_touch_exclusion_reason_updated_at();
