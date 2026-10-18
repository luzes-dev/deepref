//! OpenAlex: work metadata by DOI. Singleton lookups are free, so the key only
//! lifts the per-second rate. A key that fails downgrades the process to keyless
//! calls until restart.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

/// Requests per second with a valid key (handoff section 5.1).
pub const KEYED_RATE_PER_SECOND: u32 = 25;
/// Requests per second once the key is absent or has been downgraded.
pub const KEYLESS_RATE_PER_SECOND: u32 = 8;

/// Whether this process sends the OpenAlex key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAlexKeyState {
    Keyed,
    Keyless,
}

/// The configured key and its one-way downgrade. The flag is atomic so every clone
/// of the client sees the same state without holding a lock across a request.
pub struct OpenAlexKey {
    key: Option<String>,
    downgraded: AtomicBool,
}

impl OpenAlexKey {
    /// A blank key counts as no key.
    pub fn new(key: Option<String>) -> Self {
        Self {
            key: key.filter(|key| !key.trim().is_empty()),
            downgraded: AtomicBool::new(false),
        }
    }

    pub fn state(&self) -> OpenAlexKeyState {
        if self.key.is_some() && !self.downgraded.load(Ordering::Acquire) {
            OpenAlexKeyState::Keyed
        } else {
            OpenAlexKeyState::Keyless
        }
    }

    pub fn rate_per_second(&self) -> u32 {
        match self.state() {
            OpenAlexKeyState::Keyed => KEYED_RATE_PER_SECOND,
            OpenAlexKeyState::Keyless => KEYLESS_RATE_PER_SECOND,
        }
    }

    /// Moves a keyed process to keyless. Returns true only for the call that makes
    /// the change, and only that call logs the warning, so it fires once per process.
    pub fn downgrade(&self, reason: &'static str) -> bool {
        if self.key.is_none() {
            return false;
        }
        let first = !self.downgraded.swap(true, Ordering::AcqRel);
        if first {
            tracing::warn!(
                reason,
                rate_per_second = KEYLESS_RATE_PER_SECOND,
                "OpenAlex key unusable; continuing keyless until restart"
            );
        }
        first
    }
}

impl fmt::Debug for OpenAlexKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAlexKey")
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .field("state", &self.state())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rates_are_the_keyed_and_keyless_ceilings() {
        assert_eq!((KEYED_RATE_PER_SECOND, KEYLESS_RATE_PER_SECOND), (25, 8));
    }

    #[test]
    fn a_configured_key_starts_keyed_at_the_keyed_rate() {
        let key = OpenAlexKey::new(Some("test-key".to_owned()));
        assert_eq!(key.state(), OpenAlexKeyState::Keyed);
        assert_eq!(key.rate_per_second(), KEYED_RATE_PER_SECOND);
    }

    #[test]
    fn no_key_or_a_blank_key_is_keyless_and_has_nothing_to_downgrade() {
        for configured in [None, Some("   ".to_owned())] {
            let key = OpenAlexKey::new(configured);
            assert_eq!(key.state(), OpenAlexKeyState::Keyless);
            assert_eq!(key.rate_per_second(), KEYLESS_RATE_PER_SECOND);
            assert!(!key.downgrade("no key was sent"));
        }
    }

    #[test]
    fn a_downgrade_happens_once_and_lowers_the_rate() {
        let key = OpenAlexKey::new(Some("test-key".to_owned()));

        assert!(key.downgrade("first rejection"));
        assert!(!key.downgrade("second rejection"));

        assert_eq!(key.state(), OpenAlexKeyState::Keyless);
        assert_eq!(key.rate_per_second(), KEYLESS_RATE_PER_SECOND);
    }

    #[test]
    fn the_key_never_appears_in_debug_output() {
        let key = OpenAlexKey::new(Some("test-key".to_owned()));
        let debug = format!("{key:?}");
        assert!(!debug.contains("test-key"), "{debug}");
        assert!(debug.contains("Keyed"), "{debug}");
    }
}
