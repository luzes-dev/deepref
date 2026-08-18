use std::{
    hash::{Hash, Hasher},
    time::Duration,
};

use deepref_events::DomainPayload;
use neo4rs::{Graph, Txn, query};
use uuid::Uuid;

use crate::{
    ApplyOutcome, GraphEdge, GraphError, GraphMetrics, GraphMutation, GraphNode, ProjectGraph,
    ProjectionMetadata,
};

pub struct GraphRepository {
    graph: Graph,
    query_timeout: Duration,
}

impl GraphRepository {
    pub async fn connect(
        uri: &str,
        user: &str,
        password: &str,
        query_timeout: Duration,
    ) -> Result<Self, GraphError> {
        let graph = tokio::time::timeout(query_timeout, Graph::new(uri, user, password))
            .await
            .map_err(|_| GraphError::Timeout(query_timeout))??;
        Ok(Self {
            graph,
            query_timeout,
        })
    }

    pub async fn ping(&self) -> Result<(), GraphError> {
        self.timed(self.graph.run(query("RETURN 1"))).await
    }

    pub async fn apply_migrations(&self) -> Result<(), GraphError> {
        for source in [
            include_str!("../migrations/0001_constraints.cypher"),
            include_str!("../migrations/0002_projection_cursors.cypher"),
        ] {
            for statement in source.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                self.timed(self.graph.run(query(statement))).await?;
            }
        }
        Ok(())
    }

    pub async fn clear_projection(&self) -> Result<(), GraphError> {
        self.timed(self.graph.run(query("MATCH (n) DETACH DELETE n")))
            .await?;
        self.apply_migrations().await
    }

    pub async fn apply_mutation(
        &self,
        mutation: &GraphMutation,
    ) -> Result<ApplyOutcome, GraphError> {
        let mut tx = self.timed(self.graph.start_txn()).await?;
        let mut cursor = self
            .timed(tx.execute(
                query(
                    "MERGE (c:ProjectionCursor {entity_type:$entity_type, entity_key:$entity_key}) \
                     ON CREATE SET c.revision=-1 WITH c WHERE c.revision < $revision \
                     SET c.revision=$revision,c.event_id=$event_id,c.updated_at=datetime() \
                     RETURN c.revision AS revision",
                )
                .param("entity_type", mutation.entity_type.as_str())
                .param("entity_key", mutation.entity_key.clone())
                .param("revision", mutation.revision)
                .param("event_id", mutation.event_id.to_string()),
            ))
            .await?;
        let applied = self.timed(cursor.next(tx.handle())).await?.is_some();
        if !applied {
            self.timed(tx.commit()).await?;
            return Ok(ApplyOutcome::StaleOrDuplicate);
        }

        apply_payload(&mut tx, &mutation.payload).await?;
        self.timed(tx.run(
            query(
                "MERGE (e:ProcessedEvent {id:$id}) SET e.revision=$revision,e.processed_at=datetime()",
            )
            .param("id", mutation.event_id.to_string())
            .param("revision", mutation.revision),
        ))
        .await?;
        self.timed(tx.commit()).await?;
        Ok(ApplyOutcome::Applied)
    }

    pub async fn project_graph(
        &self,
        project_id: Uuid,
        projection: ProjectionMetadata,
    ) -> Result<ProjectGraph, GraphError> {
        const LIMIT: i64 = 2_000;
        let project_id = project_id.to_string();
        let mut node_rows = self
            .timed(
                self.graph.execute(
                    query(
                        "MATCH (:Project {id:$project_id})-[:CONTAINS]->(w:Work) \
                     RETURN w.doi AS doi,w.title AS title,w.issued_year AS issued_year, \
                     coalesce(w.total_citations,0) AS total_citations ORDER BY w.doi LIMIT $limit",
                    )
                    .param("project_id", project_id.clone())
                    .param("limit", LIMIT + 1),
                ),
            )
            .await?;
        let mut nodes = Vec::new();
        while let Some(row) = self.timed(node_rows.next()).await? {
            nodes.push(GraphNode {
                doi: row.get("doi")?,
                title: row.get("title").ok(),
                issued_year: row.get("issued_year").ok(),
                total_citations: row.get("total_citations")?,
            });
        }
        let mut edge_rows = self
            .timed(self.graph.execute(
                query(
                    "MATCH (p:Project {id:$project_id})-[:CONTAINS]->(s:Work)-[:CITES]->(t:Work) \
                     WHERE (p)-[:CONTAINS]->(t) RETURN s.doi AS source,t.doi AS target \
                     ORDER BY source,target LIMIT $limit",
                )
                .param("project_id", project_id)
                .param("limit", LIMIT + 1),
            ))
            .await?;
        let mut edges = Vec::new();
        while let Some(row) = self.timed(edge_rows.next()).await? {
            edges.push(GraphEdge {
                source: row.get("source")?,
                target: row.get("target")?,
            });
        }
        let truncated = nodes.len() > LIMIT as usize || edges.len() > LIMIT as usize;
        nodes.truncate(LIMIT as usize);
        edges.truncate(LIMIT as usize);
        Ok(ProjectGraph {
            nodes,
            edges,
            projection,
            truncated,
        })
    }

    pub async fn compute_metrics(&self, project_id: Uuid) -> Result<GraphMetrics, GraphError> {
        let mut rows = self
            .timed(
                self.graph.execute(
                    query(
                        "MATCH (p:Project {id:$project_id})-[:CONTAINS]->(w:Work) \
                 OPTIONAL MATCH (p)-[:CONTAINS]->(s:Work)-[c:CITES]->(t:Work)<-[:CONTAINS]-(p) \
                 RETURN count(DISTINCT w) AS work_count,count(DISTINCT c) AS edge_count",
                    )
                    .param("project_id", project_id.to_string()),
                ),
            )
            .await?;
        let row = self
            .timed(rows.next())
            .await?
            .ok_or(GraphError::MissingField("metrics"))?;
        Ok(GraphMetrics {
            work_count: row.get("work_count")?,
            edge_count: row.get("edge_count")?,
        })
    }

    pub async fn load_work_snapshot(
        &self,
        doi: &str,
        title: Option<&str>,
        issued_year: Option<i32>,
        total_citations: i32,
    ) -> Result<(), GraphError> {
        self.timed(self.graph.run(
            query("MERGE (w:Work {doi:$doi}) SET w.title=$title,w.issued_year=$year,w.total_citations=$total")
                .param("doi", doi)
                .param("title", title.unwrap_or_default())
                .param("year", i64::from(issued_year.unwrap_or_default()))
                .param("total", i64::from(total_citations)),
        )).await
    }

    pub async fn load_membership_snapshot(
        &self,
        project_id: Uuid,
        doi: &str,
        seed: bool,
        min_depth: i32,
    ) -> Result<(), GraphError> {
        self.timed(self.graph.run(
            query("MERGE (p:Project {id:$project}) MERGE (w:Work {doi:$doi}) MERGE (p)-[m:CONTAINS]->(w) SET m.seed=$seed,m.min_depth=$depth")
                .param("project", project_id.to_string()).param("doi", doi)
                .param("seed", seed).param("depth", i64::from(min_depth)),
        )).await
    }

    pub async fn load_citation_snapshot(
        &self,
        source: &str,
        target: &str,
    ) -> Result<(), GraphError> {
        self.timed(self.graph.run(
            query("MERGE (s:Work {doi:$source}) MERGE (t:Work {doi:$target}) MERGE (s)-[:CITES]->(t)")
                .param("source", source).param("target", target),
        )).await
    }

    pub async fn counts(&self) -> Result<GraphMetrics, GraphError> {
        let mut rows = self.timed(self.graph.execute(query(
            "MATCH (w:Work) OPTIONAL MATCH ()-[c:CITES]->() RETURN count(DISTINCT w) AS work_count,count(DISTINCT c) AS edge_count",
        ))).await?;
        let row = self
            .timed(rows.next())
            .await?
            .ok_or(GraphError::MissingField("counts"))?;
        Ok(GraphMetrics {
            work_count: row.get("work_count")?,
            edge_count: row.get("edge_count")?,
        })
    }

    pub async fn projection_hash(&self) -> Result<u64, GraphError> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let mut works = self
            .timed(
                self.graph
                    .execute(query("MATCH (w:Work) RETURN w.doi AS doi ORDER BY doi")),
            )
            .await?;
        while let Some(row) = self.timed(works.next()).await? {
            "work".hash(&mut hasher);
            row.get::<String>("doi")?.hash(&mut hasher);
        }
        let mut citations = self
            .timed(self.graph.execute(query(
                "MATCH (s:Work)-[:CITES]->(t:Work) RETURN s.doi AS source,t.doi AS target ORDER BY source,target",
            )))
            .await?;
        while let Some(row) = self.timed(citations.next()).await? {
            "citation".hash(&mut hasher);
            row.get::<String>("source")?.hash(&mut hasher);
            row.get::<String>("target")?.hash(&mut hasher);
        }
        Ok(hasher.finish())
    }

    async fn timed<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, neo4rs::Error>>,
    ) -> Result<T, GraphError> {
        tokio::time::timeout(self.query_timeout, future)
            .await
            .map_err(|_| GraphError::Timeout(self.query_timeout))?
            .map_err(GraphError::from)
    }
}

async fn apply_payload(tx: &mut Txn, payload: &DomainPayload) -> Result<(), GraphError> {
    let mutation = match payload {
        DomainPayload::WorkUpserted(value) => query(
            "MERGE (w:Work {doi:$doi}) SET w.title=$title,w.issued_year=$issued_year,w.total_citations=$total",
        ).param("doi", value.doi.clone())
         .param("title", value.title.clone().unwrap_or_default())
         .param("issued_year", i64::from(value.issued_year.unwrap_or_default()))
         .param("total", i64::from(value.total_citations)),
        DomainPayload::WorkTombstoned(value) =>
            query("MATCH (w:Work {doi:$doi}) DETACH DELETE w").param("doi", value.doi.clone()),
        DomainPayload::ProjectMembershipUpserted(value) => query(
            "MERGE (p:Project {id:$project}) MERGE (w:Work {doi:$doi}) \
             MERGE (p)-[m:CONTAINS]->(w) SET m.seed=$seed,m.min_depth=$depth",
        ).param("project", value.project_id.to_string()).param("doi", value.doi.clone())
         .param("seed", value.seed).param("depth", i64::from(value.min_depth)),
        DomainPayload::ProjectMembershipTombstoned(value) => query(
            "MATCH (:Project {id:$project})-[m:CONTAINS]->(:Work {doi:$doi}) DELETE m",
        ).param("project", value.project_id.to_string()).param("doi", value.doi.clone()),
        DomainPayload::CitationUpserted(value) => query(
            "MERGE (s:Work {doi:$source}) MERGE (t:Work {doi:$target}) MERGE (s)-[:CITES]->(t)",
        ).param("source", value.source_doi.clone()).param("target", value.target_doi.clone()),
        DomainPayload::CitationTombstoned(value) => query(
            "MATCH (:Work {doi:$source})-[c:CITES]->(:Work {doi:$target}) DELETE c",
        ).param("source", value.source_doi.clone()).param("target", value.target_doi.clone()),
        DomainPayload::UnresolvedReferenceUpserted(value) => query(
            "MERGE (r:ReferenceStub {id:$id}) SET r.raw=$raw \
             WITH r MATCH (s:Work {doi:$source}) MERGE (s)-[:HAS_UNRESOLVED_REFERENCE]->(r)",
        ).param("id", value.id.clone())
         .param("raw", value.raw_unstructured.clone().unwrap_or_default())
         .param("source", value.source_doi.clone()),
        DomainPayload::UnresolvedReferenceTombstoned(value) =>
            query("MATCH (r:ReferenceStub {id:$id}) DETACH DELETE r").param("id", value.id.clone()),
        DomainPayload::ProjectTombstoned(value) =>
            query("MATCH (p:Project {id:$id}) DETACH DELETE p").param("id", value.project_id.to_string()),
        other => return Err(GraphError::Unsupported(format!("{other:?}"))),
    };
    tx.run(mutation).await?;
    Ok(())
}
