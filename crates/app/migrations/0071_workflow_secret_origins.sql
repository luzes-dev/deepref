ALTER TABLE workflow_secrets ADD COLUMN origin text;
-- Legacy HTTP secrets have no trustworthy origin binding: require re-entry.
DELETE FROM workflow_secrets s USING workflows w
WHERE s.workflow_id=w.id AND EXISTS (
    SELECT 1 FROM jsonb_array_elements(w.draft_graph->'nodes') n
    WHERE n->>'id'=s.node_id AND n->>'type'='integration.http_request'
);
