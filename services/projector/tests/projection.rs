mod support;

#[test]
fn graph_migrations_make_event_and_cursor_identity_unique() {
    let source = support::GRAPH_MIGRATIONS.join("\n");
    assert!(source.contains("processed_event_id"));
    assert!(source.contains("projection_cursor_identity"));
}
