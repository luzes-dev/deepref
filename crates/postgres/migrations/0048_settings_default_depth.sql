-- The workspace default citation depth is 1. Depth 2 can add thousands of records, so a new
-- import starts shallow and a deeper crawl is an explicit choice.
ALTER TABLE settings ALTER COLUMN default_max_depth SET DEFAULT 1;

-- Move rows still on the old default only when nobody has changed the settings. The row is
-- created with created_at = updated_at, and every settings update moves updated_at forward, so
-- equal timestamps mean no user has touched it. A row that was ever updated keeps its value: the
-- update may have changed something else, so the depth cannot be told apart from a deliberate 2.
UPDATE settings
SET default_max_depth = 1
WHERE id = 1 AND default_max_depth = 2 AND updated_at = created_at;

-- Projects created without an explicit depth, and legacy ingestions, fall back to the same value.
-- Existing rows keep the depth they were given.
ALTER TABLE projects ALTER COLUMN default_max_depth SET DEFAULT 1;
ALTER TABLE ingestions ALTER COLUMN max_depth SET DEFAULT 1;
