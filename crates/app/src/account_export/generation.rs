//! A blocking worker owns staging until all writes and cleanup have stopped.
//! Cancellation is cooperative between bounded operations; a kernel filesystem
//! call cannot be interrupted safely. None of those calls poll on an executor.
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::ensure;
use tokio::sync::watch;

#[derive(Clone)]
pub(super) struct GenerationBudget {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}

impl GenerationBudget {
    pub(super) fn new(duration: Duration) -> Self {
        Self {
            deadline: Instant::now() + duration,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    pub(super) fn check(&self) -> anyhow::Result<()> {
        ensure!(
            !self.cancelled.load(Ordering::Acquire),
            "export generation cancelled"
        );
        ensure!(
            Instant::now() < self.deadline,
            "export generation deadline exceeded"
        );
        Ok(())
    }
}

struct CancelOnDrop {
    budget: GenerationBudget,
    signal: watch::Sender<bool>,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.budget.cancelled.store(true, Ordering::Release);
        self.signal.send_replace(true);
    }
}

pub(super) async fn run<F, Fut, T>(duration: Duration, work: F) -> anyhow::Result<T>
where
    F: FnOnce(GenerationBudget) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<T>>,
    T: Send + 'static,
{
    let budget = GenerationBudget::new(duration);
    let (signal, mut cancelled) = watch::channel(false);
    let _guard = CancelOnDrop {
        budget: budget.clone(),
        signal,
    };
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        // Handle::block_on polls this future on this blocking thread. Synchronous
        // collector/ZIP operations therefore cannot starve lease renewal tasks.
        runtime.block_on(async move {
            budget.check()?;
            let deadline = tokio::time::Instant::from_std(budget.deadline);
            tokio::select! {
                biased;
                _ = cancelled.changed() => anyhow::bail!("export generation cancelled"),
                result = tokio::time::timeout_at(deadline, work(budget.clone())) => {
                    let value = result.map_err(|_| anyhow::anyhow!("export generation deadline exceeded"))??;
                    budget.check()?;
                    Ok(value)
                }
            }
        })
    }).await?
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::account_export::zip::{self, Staging};
    use std::{io::Read, path::PathBuf};
    use tokio::sync::oneshot;

    struct DelayedRead<R>(R);
    impl<R: Read> Read for DelayedRead<R> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(Duration::from_millis(700));
            self.0.read(bytes)
        }
    }

    // A current-thread runtime makes accidental filesystem work on the
    // executor observable: the 25ms heartbeat cannot run through a 700ms read.
    #[tokio::test(flavor = "current_thread")]
    async fn final_rehash_deadline_keeps_executor_responsive_and_cleans_staging() {
        let (started, path) = oneshot::channel();
        let task = tokio::spawn(run(Duration::from_millis(300), move |budget| async move {
            let staging = Staging::new()?;
            let file = staging.path().join("archive.zip");
            std::fs::write(&file, b"archive bytes")?;
            let mut reader = DelayedRead(std::fs::File::open(file)?);
            let _ = started.send(staging.path().to_path_buf());
            zip::hash_archive(&mut reader, &budget)
        }));
        let path = path.await.unwrap();
        tokio::time::timeout(
            Duration::from_millis(200),
            tokio::time::sleep(Duration::from_millis(25)),
        )
        .await
        .expect("archive IO must leave the executor responsive");
        assert!(!task.is_finished());
        assert!(
            path.exists(),
            "the writer still owns staging during the blocking read"
        );
        let result = task.await.unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("deadline exceeded")
        );
        assert!(!path.exists(), "join completes after staging cleanup");
    }

    struct CleanupWitness {
        staging: Option<Staging>,
        finished: Option<oneshot::Sender<()>>,
    }
    impl Drop for CleanupWitness {
        fn drop(&mut self) {
            drop(self.staging.take());
            if let Some(finished) = self.finished.take() {
                let _ = finished.send(());
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_leaves_staging_with_the_writer_until_it_stops() {
        let (started, path) = oneshot::channel::<PathBuf>();
        let (finished, cleaned) = oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let task = tokio::spawn(run(Duration::from_secs(5), move |budget| async move {
            let staging = Staging::new()?;
            let path = staging.path().to_path_buf();
            let _owner = CleanupWitness {
                staging: Some(staging),
                finished: Some(finished),
            };
            let _ = started.send(path);
            held.recv()?; // deterministic stand-in for a non-interruptible syscall
            budget.check()?;
            Ok(())
        }));
        let path = path.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(
            path.exists(),
            "parent cancellation cannot remove an active writer's directory"
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), cleaned)
            .await
            .unwrap()
            .unwrap();
        assert!(!path.exists());
    }
}
