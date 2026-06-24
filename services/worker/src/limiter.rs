use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;

pub(crate) type DirectRateLimiter = RateLimiter<
    governor::state::direct::NotKeyed,
    governor::state::InMemoryState,
    governor::clock::DefaultClock,
>;

pub(crate) fn per_second(rate: u32) -> DirectRateLimiter {
    let rate = NonZeroU32::new(rate.max(1)).expect("rate is clamped to at least 1");
    RateLimiter::direct(Quota::per_second(rate))
}
