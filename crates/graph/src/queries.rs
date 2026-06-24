pub const CONSTRAINTS: &[&str] = &[
    "CREATE CONSTRAINT project_id IF NOT EXISTS FOR (p:Project) REQUIRE p.id IS UNIQUE",
    "CREATE CONSTRAINT ingestion_id IF NOT EXISTS FOR (i:Ingestion) REQUIRE i.id IS UNIQUE",
    "CREATE CONSTRAINT work_doi IF NOT EXISTS FOR (w:Work) REQUIRE w.doi IS UNIQUE",
    "CREATE CONSTRAINT author_id IF NOT EXISTS FOR (a:Author) REQUIRE a.id IS UNIQUE",
    "CREATE CONSTRAINT venue_id IF NOT EXISTS FOR (v:Venue) REQUIRE v.id IS UNIQUE",
    "CREATE CONSTRAINT reference_stub_id IF NOT EXISTS FOR (r:ReferenceStub) REQUIRE r.id IS UNIQUE",
];

pub const INTERNAL_CITATIONS_QUERY: &str = r#"
MATCH (p:Project {id: $project_id})-[m:CONTAINS]->(w:Work)
OPTIONAL MATCH (p)-[:CONTAINS]->(citing:Work)-[:CITES]->(w)
WITH m, w, count(DISTINCT citing) AS internal
SET
  m.internal_citations = internal,
  m.total_citations = coalesce(w.total_citations, 0),
  m.metrics_computed_at = datetime()
RETURN
  w.doi AS doi,
  w.title AS title,
  internal AS internal_citations,
  coalesce(w.total_citations, 0) AS total_citations
ORDER BY internal_citations DESC, total_citations DESC
"#;

pub const OUTBOUND_INTERNAL_REFERENCES_QUERY: &str = r#"
MATCH (p:Project {id: $project_id})-[m:CONTAINS]->(w:Work)
OPTIONAL MATCH (w)-[:CITES]->(referenced:Work)<-[:CONTAINS]-(p)
WITH m, w, count(DISTINCT referenced) AS outbound
SET m.outbound_internal_references = outbound
RETURN w.doi AS doi, outbound
ORDER BY outbound DESC
"#;

pub const GRAPH_EXPORT_QUERY: &str = r#"
MATCH (p:Project {id: $project_id})-[:CONTAINS]->(source:Work)-[r:CITES]->(target:Work)
WHERE (p)-[:CONTAINS]->(target)
RETURN
  source.doi AS source,
  target.doi AS target,
  source.title AS source_title,
  target.title AS target_title,
  coalesce(source.total_citations, 0) AS source_total_citations,
  coalesce(target.total_citations, 0) AS target_total_citations
"#;
