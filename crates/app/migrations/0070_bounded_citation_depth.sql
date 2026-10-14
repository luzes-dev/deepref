-- Preserve historical crawl depth without permitting unbounded new run inputs.
ALTER TABLE settings ADD CONSTRAINT settings_depth_bounded CHECK (default_max_depth BETWEEN 0 AND 3);
ALTER TABLE projects ADD CONSTRAINT projects_depth_bounded CHECK (default_max_depth BETWEEN 0 AND 3);
ALTER TABLE ingestions ADD COLUMN historical_max_depth INTEGER;
ALTER TABLE acquisition_runs ADD COLUMN historical_max_depth INTEGER;
UPDATE ingestions SET historical_max_depth = max_depth WHERE max_depth > 3;
UPDATE acquisition_runs SET historical_max_depth = max_depth WHERE max_depth > 3;
ALTER TABLE ingestions ADD CONSTRAINT ingestions_depth_bounded
    CHECK (max_depth BETWEEN 0 AND 3 OR (historical_max_depth IS NOT NULL AND max_depth = historical_max_depth));
ALTER TABLE acquisition_runs ADD CONSTRAINT acquisition_runs_depth_bounded
    CHECK (max_depth BETWEEN 0 AND 3 OR (historical_max_depth IS NOT NULL AND max_depth = historical_max_depth));
