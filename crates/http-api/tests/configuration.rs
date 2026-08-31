mod support;

#[test]
fn local_runtime_has_bounded_pools_and_timeouts() {
    let runtime = support::local_runtime();
    assert!(runtime.database.pool_min <= runtime.database.pool_max);
    assert!(!runtime.database.acquire_timeout.is_zero());
    assert!(!runtime.telemetry.shutdown_deadline.is_zero());
}
