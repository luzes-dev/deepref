pub const GRAPH_MIGRATIONS: [&str; 2] = [
    include_str!("../../../../crates/graph/migrations/0001_constraints.cypher"),
    include_str!("../../../../crates/graph/migrations/0002_projection_cursors.cypher"),
];
