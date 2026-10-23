use std::time::{Duration, Instant};

use super::CheckResult;

/// Cooperative absolute monotonic deadline. Check between bounded operations;
/// this cannot preempt an in-progress synchronous filesystem call or parse.
pub(super) struct Deadline {
    at: Instant,
    #[cfg(test)]
    clock: Option<Box<dyn Fn() -> Instant>>,
}

impl Deadline {
    pub fn after(duration: Duration) -> Self {
        Self {
            at: Instant::now() + duration,
            #[cfg(test)]
            clock: None,
        }
    }

    pub fn check(&self) -> CheckResult<()> {
        #[cfg(test)]
        let now = self
            .clock
            .as_ref()
            .map_or_else(Instant::now, |clock| clock());
        #[cfg(not(test))]
        let now = Instant::now();
        if now >= self.at {
            Err("overall_timeout")
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    pub fn with_clock(at: Instant, clock: impl Fn() -> Instant + 'static) -> Self {
        Self {
            at,
            clock: Some(Box::new(clock)),
        }
    }
}
