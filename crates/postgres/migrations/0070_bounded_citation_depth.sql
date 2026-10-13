-- Refuse unsafe historical values rather than silently changing crawl semantics.
ALTER TABLE settings ADD CONSTRAINT settings_depth_bounded CHECK (default_max_depth BETWEEN 0 AND 3);
ALTER TABLE projects ADD CONSTRAINT projects_depth_bounded CHECK (default_max_depth BETWEEN 0 AND 3);
ALTER TABLE ingestions ADD CONSTRAINT ingestions_depth_bounded CHECK (max_depth BETWEEN 0 AND 3);
ALTER TABLE acquisition_runs ADD CONSTRAINT acquisition_runs_depth_bounded CHECK (max_depth BETWEEN 0 AND 3);
