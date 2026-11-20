//! Typed identifiers, so a session id can never be passed where a run id is expected.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! numeric_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

numeric_id!(
    /// A conversation with the agent.
    SessionId,
    "ses"
);
numeric_id!(
    /// One message inside a session.
    MessageId,
    "msg"
);
numeric_id!(
    /// One execution of the agent (a user turn and everything the agent does in reply).
    RunId,
    "run"
);

/// A tool call's id. Providers mint these (as strings), so it is not numeric.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ToolCallId(pub String);

impl fmt::Display for ToolCallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Monotonic id source. Thread-safe so engines on worker threads can mint ids too.
#[derive(Debug)]
pub struct IdGen(AtomicU64);

impl Default for IdGen {
    fn default() -> Self {
        Self::new()
    }
}

impl IdGen {
    pub const fn new() -> Self {
        IdGen(AtomicU64::new(1))
    }

    /// Ids starting at `first`.
    pub const fn starting_at(first: u64) -> Self {
        IdGen(AtomicU64::new(first))
    }

    /// Ids starting at the current time in milliseconds, for ids that must not repeat across
    /// launches (saved sessions): a later launch starts past every id an earlier one minted,
    /// unless that one minted more than one id per millisecond it ran. Ids grow with time.
    pub fn from_clock() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX / 2));
        Self::starting_at(now)
    }

    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic_and_display_with_prefix() {
        let ids = IdGen::new();
        let a = SessionId(ids.next());
        let b = SessionId(ids.next());
        assert!(b > a);
        assert_eq!(a.to_string(), "ses1");
        assert_eq!(RunId(7).to_string(), "run7");
    }

    #[test]
    fn clock_ids_start_past_the_small_ones() {
        let ids = IdGen::from_clock();
        let first = ids.next();
        assert!(first > 1_600_000_000_000, "milliseconds since 1970");
        assert_eq!(ids.next(), first + 1);
    }
}
