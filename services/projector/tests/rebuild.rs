use deepref_projector::rebuild::REBUILD_STAGES;

#[test]
fn rebuild_has_all_required_ordered_stages() {
    assert_eq!(REBUILD_STAGES.len(), 8);
    assert_eq!(REBUILD_STAGES[0], "advisory_lock");
    assert_eq!(REBUILD_STAGES[7], "ready_resume");
}
