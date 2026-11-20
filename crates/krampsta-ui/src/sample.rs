//! Example content for stills and tests: sessions, a project folder and proposed changes for the
//! headless scenes of the changes panel.

use krampsta_core::{FileChange, IdGen, MessageRole, Session, SessionId};

const WELCOME: &str = "Welcome to **Krampsta**, a native AI coding agent built with Rust and Rune.\n\n\
This first version runs a *simulated* agent, so you can try the whole flow:\n\n\
- Create a session with **New session** (or `Ctrl+N` / `⌘N`).\n\
- Type below and press **Enter** to send (`Shift+Enter` adds a line).\n\
- Watch the status pill go from *Running* to *Completed*.\n\
- Start a message with `/fail` to see a failed run, or press **Stop** while it runs.\n\n\
The panel on the right previews file changes; toggle it from the toolbar.";

/// The sessions stills show (`KrampstaApp::with_sample_sessions`). The first is selected.
pub fn sessions(ids: &IdGen) -> Vec<Session> {
    let mut welcome = Session::with_title(SessionId(ids.next()), "Welcome to Krampsta");
    welcome.push_message(MessageRole::Assistant, WELCOME);

    let mut retry = Session::with_title(SessionId(ids.next()), "Add jitter to retry backoff");
    retry.push_message(MessageRole::User, "Add jitter to the retry backoff so clients spread out.");
    retry.push_message(
        MessageRole::Assistant,
        "I updated `Backoff::next` to take a random roll and spread each wait by ±20%. \
         The changes are in the panel on the right.",
    );
    retry.status = krampsta_core::AgentStatus::Completed;

    vec![welcome, retry]
}

/// The project folder stills show.
pub fn project() -> crate::projects::ProjectInfo {
    crate::projects::ProjectInfo { root: "/Users/you/code/krampsta".into(), name: "krampsta".into() }
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

const OLD_TOML: &str = "[retry]
base_ms = 100
max_ms = 1000

[http]
timeout_ms = 5000
keep_alive = true
pool = 8
user_agent = \"krampsta\"

[log]
level = \"info\"
";
const NEW_TOML: &str = "[retry]
base_ms = 100
max_ms = 1000
jitter = 0.2

[http]
timeout_ms = 5000
keep_alive = true
pool = 8
user_agent = \"krampsta\"

[log]
level = \"debug\"
";

const NEW_DOC: &str = "# Retries

Waits grow from `base_ms` to `max_ms`, give or take `jitter`.
";

/// Changes as the agent proposes them: edits to two files and a new one.
pub fn changes() -> Vec<FileChange> {
    vec![
        FileChange { path: "src/net/backoff.rs".into(), before: Some(OLD_RS.into()), after: NEW_RS.into() },
        FileChange { path: "config/retry.toml".into(), before: Some(OLD_TOML.into()), after: NEW_TOML.into() },
        FileChange { path: "docs/retry.md".into(), before: None, after: NEW_DOC.into() },
    ]
}
