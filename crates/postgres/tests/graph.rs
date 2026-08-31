use anyhow::{Result, ensure};
use chrono::Utc;
use deepref_graph::GraphEdge;
use deepref_postgres::{load_project_graph, migrate, recompute_project_metrics};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .unwrap_or_else(|error| {
            panic!("DATABASE_URL is set but PostgreSQL is unavailable: {error}")
        });
    migrate(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to apply PostgreSQL migrations: {error}"));
    Some(pool)
}

#[tokio::test]
async fn postgres_graph_is_deterministic_and_keeps_identifier_free_reports() {
    let Some(pool) = database().await else {
        return;
    };

    let project_id = Uuid::new_v4();
    let report_a = Uuid::new_v4();
    let report_b = Uuid::new_v4();
    let report_without_doi = Uuid::new_v4();
    let external_report = Uuid::new_v4();
    let reports = [report_a, report_b, report_without_doi, external_report];

    let result = seed_and_assert(
        &pool,
        project_id,
        report_a,
        report_b,
        report_without_doi,
        external_report,
    )
    .await;

    sqlx::query("DELETE FROM projection_state WHERE project_id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("fixture projection state should be removable");
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("fixture project should be removable");
    sqlx::query("DELETE FROM reports WHERE id = ANY($1)")
        .bind(reports.as_slice())
        .execute(&pool)
        .await
        .expect("fixture reports should be removable");

    result.expect("PostgreSQL graph must match the deterministic fixture");
}

async fn seed_and_assert(
    pool: &PgPool,
    project_id: Uuid,
    report_a: Uuid,
    report_b: Uuid,
    report_without_doi: Uuid,
    external_report: Uuid,
) -> Result<()> {
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'postgres graph fixture')")
        .bind(project_id)
        .execute(pool)
        .await?;

    for (id, title, citations, year) in [
        (report_a, "A", 100_i64, 2024_i32),
        (report_b, "B", 10_i64, 2020_i32),
        (report_without_doi, "No DOI", 0_i64, 2022_i32),
        (external_report, "External", 1_i64, 2021_i32),
    ] {
        sqlx::query(
            "INSERT INTO reports (id,title,publication_year,total_citations,references_count,raw) \
             VALUES ($1,$2,$3,$4,2,'{}'::jsonb)",
        )
        .bind(id)
        .bind(title)
        .bind(year)
        .bind(citations)
        .execute(pool)
        .await?;
    }

    sqlx::query(
        "INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value) \
         VALUES ($1,$2,'doi','10.5555/graph-a','10.5555/graph-a'),\
                ($3,$4,'doi','10.5555/graph-b','10.5555/graph-b')",
    )
    .bind(Uuid::new_v4())
    .bind(report_a)
    .bind(Uuid::new_v4())
    .bind(report_b)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2),($1,$3),($1,$4)",
    )
    .bind(project_id)
    .bind(report_a)
    .bind(report_b)
    .bind(report_without_doi)
    .execute(pool)
    .await?;

    // A->B, A->No DOI, and No DOI->B are internal. The external edge is
    // deliberately excluded from the project graph and from degree metrics.
    for (source, target) in [
        (report_a, report_b),
        (report_a, report_without_doi),
        (report_without_doi, report_b),
        (report_a, external_report),
    ] {
        sqlx::query(
            "INSERT INTO citations (project_id,source_report_id,target_report_id,legacy_source_doi,legacy_target_doi) \
             VALUES ($1,$2,$3,'fixture-source','fixture-target')",
        )
        .bind(project_id)
        .bind(source)
        .bind(target)
        .execute(pool)
        .await?;
    }

    recompute_project_metrics(pool, project_id).await?;
    let graph =
        load_project_graph(pool, project_id, deepref_graph::GraphFieldSelection::all()).await?;

    ensure!(graph.nodes.len() == 3, "expected three project nodes");
    let mut expected_edges = vec![
        GraphEdge {
            source: report_a,
            target: report_b,
        },
        GraphEdge {
            source: report_a,
            target: report_without_doi,
        },
        GraphEdge {
            source: report_without_doi,
            target: report_b,
        },
    ];
    expected_edges.sort_by_key(|edge| (edge.source, edge.target));
    ensure!(
        graph.edges == expected_edges,
        "UUID edges must be sorted and project-bounded: {:?}",
        graph.edges
    );
    ensure!(
        graph.nodes.iter().any(|node| {
            node.report_id == report_without_doi
                && node.doi.is_none()
                && node.title.as_deref() == Some("No DOI")
        }),
        "identifier-free report must remain a graph node"
    );
    ensure!(
        graph.nodes.iter().all(|node| {
            node.screening.as_ref().is_some_and(|screening| {
                screening.title_abstract_status == "unscreened"
                    && screening.final_status == "unscreened"
            }) && node
                .study
                .as_ref()
                .is_some_and(|study| study.study_id.is_none())
        }),
        "graph-only projects must receive neutral review and ungrouped overlays"
    );

    let metrics = sqlx::query(
        "SELECT report_id,internal_citations,outbound_internal_references,rank_score,metrics_computed_at \
         FROM project_reports WHERE project_id=$1 ORDER BY report_id",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    ensure!(
        metrics.len() == 3,
        "expected metrics for every project report"
    );
    for row in metrics {
        let id: Uuid = row.get("report_id");
        let expected = match id {
            id if id == report_a => (0, 2),
            id if id == report_b => (2, 0),
            id if id == report_without_doi => (1, 1),
            _ => anyhow::bail!("unexpected project report {id}"),
        };
        ensure!(
            (
                row.get::<i64, _>("internal_citations"),
                row.get::<i64, _>("outbound_internal_references")
            ) == expected,
            "degree metrics changed for {id}"
        );
        ensure!(
            row.get::<Option<chrono::DateTime<Utc>>, _>("metrics_computed_at")
                .is_some()
        );
    }

    let snapshot = sqlx::query(
        "SELECT work_count,edge_count,payload FROM metric_snapshots WHERE project_id=$1 ORDER BY metrics_as_of DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    ensure!(
        snapshot.get::<i64, _>("work_count") == 3
            && snapshot.get::<i64, _>("edge_count") == 3
            && snapshot.get::<serde_json::Value, _>("payload")
                == serde_json::json!({"work_count": 3, "edge_count": 3}),
        "metric snapshot must match the bounded internal UUID graph"
    );
    let projection = sqlx::query(
        "SELECT state,lag,last_success_at FROM projection_state WHERE projection_name='postgres_graph' AND project_id=$1",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    ensure!(
        projection.get::<String, _>("state") == "ready"
            && projection.get::<i64, _>("lag") == 0
            && projection
                .get::<Option<chrono::DateTime<Utc>>, _>("last_success_at")
                .is_some(),
        "recompute must publish coherent PostgreSQL projection freshness"
    );

    let max_total = (100_f64 + 1.0).log10();
    let current_year: i32 = sqlx::query_scalar("SELECT EXTRACT(YEAR FROM CURRENT_DATE)::int")
        .fetch_one(pool)
        .await?;
    let freshness_a = 1.0 / (1.0 + ((current_year - 2024).max(0) as f64 / 10.0));
    let expected_rank_a = 0.45 * ((100_f64 + 1.0).log10() / max_total)
        + 0.40 * (0.0 / 2.0)
        + 0.10
        + 0.05 * freshness_a;
    let rank_a: f64 = sqlx::query_scalar(
        "SELECT rank_score FROM project_reports WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_a)
    .fetch_one(pool)
    .await?;
    ensure!(
        (rank_a - expected_rank_a).abs() < 1e-9,
        "legacy rank formula changed: {rank_a} != {expected_rank_a}"
    );

    let first_timestamp: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT metrics_computed_at FROM project_reports WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_a)
    .fetch_one(pool)
    .await?;
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    recompute_project_metrics(pool, project_id).await?;
    let second_timestamp: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT metrics_computed_at FROM project_reports WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_a)
    .fetch_one(pool)
    .await?;
    ensure!(
        second_timestamp > first_timestamp,
        "recompute must advance graph freshness"
    );
    Ok(())
}
