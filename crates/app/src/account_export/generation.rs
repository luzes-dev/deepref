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

struct CancelOnDrop(GenerationBudget);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::Release);
    }
}

/// Staging is owned outside the cancellable future. A successful filesystem
/// removal precedes its durable acknowledgement, even if the caller is dropped.
pub(super) async fn run_staged<F, Fut, A, Ack, T>(
    attempt: uuid::Uuid,
    duration: Duration,
    work: F,
    ack: A,
) -> anyhow::Result<T>
where
    F: FnOnce(GenerationBudget, std::path::PathBuf) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<T>>,
    A: FnOnce() -> Ack + Send + 'static,
    Ack: Future<Output = anyhow::Result<()>>,
    T: Send + 'static,
{
    let budget = GenerationBudget::new(duration);
    let _guard = CancelOnDrop(budget.clone());
    let runtime = tokio::runtime::Handle::current();
    let deadline = tokio::time::Instant::from_std(budget.deadline);
    let owned = tokio::task::spawn_blocking(move || {
        let staging = super::zip::Staging::for_attempt(attempt)?;
        let path = staging.path().to_path_buf();
        let result = runtime.block_on(async move {
            budget.check()?;
            // Never drop an in-flight adapter future merely because the caller
            // expires. It can own background filesystem/multipart operations.
            let value = work(budget.clone(), path).await?;
            budget.check()?;
            Ok(value)
        });
        staging.cleanup()?;
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), ack()).await? })?;
        result
    });
    // Dropping a JoinHandle detaches the owned worker; its real completion and
    // unlink still precede ACK. Deadline is failure, not evidence of cleanup.
    tokio::time::timeout_at(deadline, owned)
        .await
        .map_err(|_| anyhow::anyhow!("export generation deadline exceeded"))??
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::account_export::zip;
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
        let (acknowledge, acknowledged) = oneshot::channel();
        let task = tokio::spawn(run_staged(
            uuid::Uuid::new_v4(),
            Duration::from_millis(300),
            move |budget, staging| async move {
                let file = staging.join("archive.zip");
                std::fs::write(&file, b"archive bytes")?;
                let mut reader = DelayedRead(std::fs::File::open(file)?);
                let _ = started.send(staging);
                zip::hash_archive(&mut reader, &budget)
            },
            move || async move {
                let _ = acknowledge.send(());
                Ok(())
            },
        ));
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
        assert!(
            path.exists(),
            "caller deadline cannot remove staging still owned by the blocking read"
        );
        tokio::time::timeout(Duration::from_secs(2), acknowledged)
            .await
            .unwrap()
            .unwrap();
        assert!(!path.exists(), "the writer ACK follows actual cleanup");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_leaves_staging_with_the_writer_until_it_stops() {
        let (started, path) = oneshot::channel::<PathBuf>();
        let (acknowledge, acknowledged) = oneshot::channel();
        let (cleanup_path, path_to_check) = oneshot::channel::<PathBuf>();
        let (release, held) = std::sync::mpsc::channel();
        let task = tokio::spawn(run_staged(
            uuid::Uuid::new_v4(),
            Duration::from_secs(5),
            move |budget, path| async move {
                std::fs::write(path.join("private"), b"scientific bytes")?;
                let _ = cleanup_path.send(path.clone());
                let _ = started.send(path);
                held.recv()?;
                budget.check()?;
                Ok(())
            },
            move || async move {
                let path = path_to_check.await?;
                anyhow::ensure!(!path.exists(), "ACK cannot precede the staging unlink");
                let _ = acknowledge.send(());
                Ok(())
            },
        ));
        let path = path.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(
            path.join("private").is_file(),
            "an active writer retains its staging"
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), acknowledged)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !path.exists(),
            "durable stopped ACK is only permitted after unlink"
        );
    }

    struct WorkDrop(Option<oneshot::Sender<()>>);
    impl Drop for WorkDrop {
        fn drop(&mut self) {
            if let Some(signal) = self.0.take() {
                let _ = signal.send(());
            }
        }
    }
    struct OperationStopped(Arc<AtomicBool>);
    impl Drop for OperationStopped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelling_caller_does_not_drop_an_outstanding_adapter_operation() {
        let (started, path) = oneshot::channel::<PathBuf>();
        let (release, held) = oneshot::channel::<()>();
        let (dropped, mut work_dropped) = oneshot::channel::<()>();
        let (acknowledge, acknowledged) = oneshot::channel::<()>();
        let stopped = Arc::new(AtomicBool::new(false));
        let operation_stopped = stopped.clone();
        let task = tokio::spawn(run_staged(
            uuid::Uuid::new_v4(),
            Duration::from_secs(5),
            move |budget, path| async move {
                let _drop = WorkDrop(Some(dropped));
                let late_path = path.clone();
                let operation = tokio::spawn(async move {
                    let _stopped = OperationStopped(operation_stopped);
                    held.await?;
                    // A storage adapter can own background work which keeps
                    // running when its joining future is dropped.
                    std::fs::create_dir_all(&late_path)?;
                    std::fs::write(late_path.join("late-output"), b"scientific bytes")?;
                    Ok::<_, anyhow::Error>(())
                });
                let _ = started.send(path);
                operation.await??;
                budget.check()?;
                Ok(())
            },
            move || async move {
                anyhow::ensure!(
                    stopped.load(Ordering::Acquire),
                    "an active adapter operation cannot receive a stopped ACK"
                );
                let _ = acknowledge.send(());
                Ok(())
            },
        ));
        let path = path.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut work_dropped)
                .await
                .is_err(),
            "caller cancellation must leave the actual adapter future owned and polled"
        );
        assert!(
            path.exists(),
            "staging stays owned while the adapter is active"
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), acknowledged)
            .await
            .unwrap()
            .unwrap();
        assert!(!path.exists());
    }
}
