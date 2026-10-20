//! Own a handler future while its database lease is renewed. No nested task
//! survives cancellation of the processor that owns this future.

use std::{any::Any, future::Future, time::Duration};

use tokio::time::Instant;

pub(crate) async fn run<T, F, R, RF>(lease: Duration, mut renew: R, work: F) -> anyhow::Result<T>
where
    T: Send,
    F: Future<Output = T> + Send,
    R: FnMut() -> RF + Send,
    RF: Future<Output = anyhow::Result<bool>> + Send,
{
    anyhow::ensure!(!lease.is_zero(), "handler lease must be positive");
    let sent_at = Instant::now();
    let renewed = tokio::time::timeout_at(sent_at + lease, renew()).await??;
    anyhow::ensure!(renewed, "handler claim ownership was lost");
    tokio::pin!(work);
    tokio::select! {
        biased;
        result = heartbeat(lease, sent_at + lease, renew) => {
            result?;
            anyhow::bail!("handler heartbeat stopped")
        },
        result = &mut work => Ok(result),
    }
}

async fn heartbeat<R, RF>(
    lease: Duration,
    mut held_until: Instant,
    mut renew: R,
) -> anyhow::Result<()>
where
    R: FnMut() -> RF + Send,
    RF: Future<Output = anyhow::Result<bool>> + Send,
{
    let interval = (lease / 3).max(Duration::from_millis(1));
    loop {
        tokio::time::sleep_until(held_until - lease + interval).await;
        let sent_at = Instant::now();
        let renewed = tokio::time::timeout_at(held_until, renew())
            .await
            .map_err(|_| anyhow::anyhow!("handler lease renewal timed out before expiry"))??;
        anyhow::ensure!(renewed, "handler claim ownership was lost");
        held_until = sent_at + lease;
    }
}

pub(crate) fn panic_message(payload: Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_renews_throughout_a_long_handler() {
        let renewals = AtomicUsize::new(0);
        let result = run(
            Duration::from_millis(60),
            || {
                renewals.fetch_add(1, Ordering::SeqCst);
                async { Ok(true) }
            },
            async {
                tokio::time::sleep(Duration::from_millis(220)).await;
                42
            },
        )
        .await;
        assert!(matches!(result, Ok(42)));
        assert!(renewals.load(Ordering::SeqCst) >= 4);
    }

    #[tokio::test(start_paused = true)]
    async fn losing_ownership_drops_the_handler_future() {
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(Arc::clone(&dropped));
        let renewals = AtomicUsize::new(0);
        let result = run(
            Duration::from_millis(60),
            || {
                let held = renewals.fetch_add(1, Ordering::SeqCst) == 0;
                async move { Ok(held) }
            },
            async move {
                let _guard = guard;
                std::future::pending::<()>().await;
            },
        )
        .await;
        assert!(result.is_err());
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn ownership_loss_wins_when_work_and_heartbeat_are_ready_together() {
        let renewals = AtomicUsize::new(0);
        let completed = AtomicBool::new(false);
        let result = run(
            Duration::from_millis(60),
            || {
                let owned = renewals.fetch_add(1, Ordering::SeqCst) == 0;
                async move { Ok(owned) }
            },
            async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                completed.store(true, Ordering::SeqCst);
            },
        )
        .await;
        assert!(result.is_err());
        assert!(!completed.load(Ordering::SeqCst));
    }
}
