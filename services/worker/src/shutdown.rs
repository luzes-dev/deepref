pub async fn wait_for_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = async {
                if let Some(stream) = terminate.as_mut() {
                    stream.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

/// Resolve when `signal` reports (or drops) shutdown, so loops that take a
/// shutdown future can watch the shared signal channel.
pub async fn on_signal(signal: tokio::sync::watch::Receiver<bool>) {
    let mut signal = signal;
    loop {
        if *signal.borrow() {
            return;
        }
        if signal.changed().await.is_err() {
            return;
        }
    }
}

/// Close the reserved control pool after all owned processors stop.
pub async fn close_control_pool(pool: &sqlx::PgPool, deadline: std::time::Duration) {
    if tokio::time::timeout(deadline, pool.close()).await.is_err() {
        // Pool::close marks the pool closed before waiting for holders. No new
        // control work is admitted; a remaining holder must not hang exit.
        tracing::warn!("worker control pool close deadline exceeded");
    }
}
