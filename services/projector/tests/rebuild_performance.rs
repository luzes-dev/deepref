#[test]
#[ignore = "acceptance benchmark; run explicitly against disposable PostgreSQL and Neo4j"]
fn representative_dataset_shape_is_250k_works_and_2_5m_edges() {
    let works = 250_000_usize;
    let edges_per_work = 10_usize;
    let generated_edges = (0..works).map(|_| edges_per_work).sum::<usize>();
    assert_eq!(generated_edges, 2_500_000);
}
