use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use chrono::Utc;
use deepref_domain::ProjectId;
use uuid::Uuid;

use crate::{
    ReviewError, ReviewFuture, ReviewRunId, ReviewRunSnapshot, ReviewRunState, ReviewScheduler,
    ScheduleReviewRun,
};

/// In-memory adapter used to exercise the same scheduling port as PostgreSQL.
#[doc(hidden)]
#[derive(Default)]
pub struct MemoryReviewScheduler {
    next_id: AtomicU64,
    runs: Mutex<BTreeMap<Uuid, ReviewRunSnapshot>>,
}

impl ReviewScheduler for MemoryReviewScheduler {
    type Error = ReviewError;

    fn schedule<'a>(&'a self, command: ScheduleReviewRun) -> ReviewFuture<'a, ReviewRunSnapshot> {
        Box::pin(async move {
            command.validate()?;
            let sequence = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            let id = ReviewRunId::new(Uuid::from_u128(u128::from(sequence)))?;
            let snapshot = ReviewRunSnapshot {
                id,
                project_id: command.project_id,
                definition: command.definition,
                subject: command.subject,
                origin: command.origin,
                state: ReviewRunState::Queued,
                created_at: Utc::now(),
                started_at: None,
                finished_at: None,
            };
            self.runs
                .lock()
                .map_err(|_| ReviewError::Persistence("memory scheduler lock poisoned".to_owned()))?
                .insert(id.as_uuid(), snapshot.clone());
            Ok(snapshot)
        })
    }

    fn get<'a>(
        &'a self,
        project_id: ProjectId,
        run_id: ReviewRunId,
    ) -> ReviewFuture<'a, ReviewRunSnapshot> {
        Box::pin(async move {
            let snapshot = self
                .runs
                .lock()
                .map_err(|_| ReviewError::Persistence("memory scheduler lock poisoned".to_owned()))?
                .get(&run_id.as_uuid())
                .filter(|run| run.project_id == project_id)
                .cloned()
                .ok_or_else(|| ReviewError::Persistence("review run was not found".to_owned()))?;
            Ok(snapshot)
        })
    }
}

#[cfg(test)]
mod tests {
    use deepref_domain::{Actor, ActorKind, RecordId, ReportId};

    use super::*;
    use crate::{ReviewDefinitionKey, ReviewOrigin, ReviewSubject};

    #[tokio::test]
    async fn memory_adapter_uses_the_public_scheduler_port_and_enforces_project_scope() {
        let scheduler = MemoryReviewScheduler::default();
        let project_id = ProjectId::new(Uuid::from_u128(1));
        let scheduled = scheduler
            .schedule(ScheduleReviewRun {
                project_id,
                definition: ReviewDefinitionKey::DuplicateDetection,
                subject: ReviewSubject::DuplicateDetection {
                    record_id: RecordId::new(Uuid::from_u128(2)),
                    candidate_report_id: ReportId::new(Uuid::from_u128(3)),
                },
                origin: ReviewOrigin::ReviewerRequested,
                actor: Actor::new(ActorKind::User, "reviewer").expect("actor"),
            })
            .await
            .expect("run schedules");
        assert_eq!(
            scheduler
                .get(project_id, scheduled.id)
                .await
                .expect("run loads"),
            scheduled
        );
        assert!(
            scheduler
                .get(ProjectId::new(Uuid::from_u128(99)), scheduled.id)
                .await
                .is_err()
        );
    }
}
