//! Example content the first version ships with: seeded sessions and a sample diff for the
//! changes panel. Replaced by real history and real file changes in later milestones.

use krune_core::{IdGen, MessageRole, Session, SessionId};
use rune_kit::diff::{DiffState, FileDiff};

const WELCOME: &str = "Welcome to **Krune**, a native AI coding agent built with Rust and Rune.\n\n\
This first version runs a *simulated* agent, so you can try the whole flow:\n\n\
- Create a session with **New session** (or `Ctrl+N` / `⌘N`).\n\
- Type below and press **Enter** to send (`Shift+Enter` adds a line).\n\
- Watch the status pill go from *Running* to *Completed*.\n\
- Start a message with `/fail` to see a failed run, or press **Stop** while it runs.\n\n\
The panel on the right previews file changes; toggle it from the toolbar.";

/// The sessions a fresh install opens with. The first is selected.
pub fn sessions(ids: &IdGen) -> Vec<Session> {
    let mut welcome = Session::with_title(SessionId(ids.next()), "Welcome to Krune");
    welcome.push_message(MessageRole::Assistant, WELCOME);

    let mut retry = Session::with_title(SessionId(ids.next()), "Add jitter to retry backoff");
    retry.push_message(MessageRole::User, "Add jitter to the retry backoff so clients spread out.");
    retry.push_message(
        MessageRole::Assistant,
        "I updated `Backoff::next` to take a random roll and spread each wait by ±20%. \
         The changes are in the panel on the right.",
    );
    retry.status = krune_core::AgentStatus::Completed;

    vec![welcome, retry]
}

const OLD_RS: &str = "use std::time::Duration;

/// How long to wait before trying a request again.
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Backoff {
        Backoff { base, max, attempt: 0 }
    }

    /// The wait before the next attempt.
    pub fn next(&mut self) -> Duration {
        let wait = self.base * 2u32.pow(self.attempt);
        self.attempt += 1;
        wait.min(self.max)
    }
}
";

const NEW_RS: &str = "use std::time::Duration;

/// How long to wait before trying a request again, with jitter so clients spread out.
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
    jitter: f32,
}

impl Backoff {
    pub fn new(base: Duration, max: Duration) -> Backoff {
        Backoff { base, max, attempt: 0, jitter: 0.2 }
    }

    /// The wait before the next attempt, give or take `jitter` of it.
    pub fn next(&mut self, roll: f32) -> Duration {
        let wait = self.base.saturating_mul(2u32.saturating_pow(self.attempt));
        self.attempt += 1;
        wait.min(self.max).mul_f32(1.0 + self.jitter * (roll * 2.0 - 1.0))
    }
}
";

const OLD_TOML: &str = "[retry]\nbase_ms = 100\nmax_ms = 1000\n";
const NEW_TOML: &str = "[retry]\nbase_ms = 100\nmax_ms = 1000\njitter = 0.2\n";

/// Sample changes for the diff panel.
pub fn diff() -> DiffState {
    let mut diff = DiffState::new(vec![
        FileDiff::from_texts("src/net/backoff.rs", OLD_RS, NEW_RS),
        FileDiff::from_texts("config/retry.toml", OLD_TOML, NEW_TOML),
    ]);
    // The panel is narrow: wrap long lines instead of scrolling sideways.
    diff.set_wrap(true);
    diff
}
