//! The tool servers the agent can reach (MCP servers, in `krampsta-mcp`), as the UI shows
//! them: each one's name, how it is reached and where it is. The UI never talks to a server; it
//! reads [`ToolServers::statuses`] when [`ToolServers::version`] moves, woken by the notifier.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::Notifier;

/// Where a server is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerState {
    Connecting,
    /// Connected; the names of the tools it offers.
    Ready {
        tools: Vec<String>,
    },
    /// Why it couldn't be reached.
    Failed(String),
    /// Configured but not started.
    Disabled,
}

/// One configured server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerStatus {
    pub name: String,
    /// How it is reached, in a line: its command, or its URL. Never its env values or headers.
    pub reached: String,
    pub state: ServerState,
}

/// How a new server is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerKind {
    /// A program Krampsta starts (stdio).
    Command,
    /// A server at a URL (Streamable HTTP).
    Url,
}

/// A server the user adds, as the form holds it; the servers check it.
#[derive(Clone, PartialEq, Eq)]
pub struct NewServer {
    pub name: String,
    pub kind: ServerKind,
    /// The command line, or the URL.
    pub target: String,
    /// One per line: `KEY=value` environment variables for a command, `Name: value` headers for
    /// a URL.
    pub extra: String,
}

/// `extra` often holds tokens: Debug shows only how long it is.
impl std::fmt::Debug for NewServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewServer")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("target", &self.target)
            .field("extra_lines", &self.extra.lines().count())
            .finish()
    }
}

/// The configured servers while Krampsta runs. Cheap to ask: no I/O, no waiting.
pub trait ToolServers: Send + Sync + 'static {
    /// Grows whenever a server's state changes.
    fn version(&self) -> u64;

    /// Every server and where it is, in the order configured.
    fn statuses(&self) -> Vec<ServerStatus>;

    /// The file they are configured in, for the UI to point at.
    fn config_file(&self) -> Option<&Path>;

    /// Called whenever a server's state changes, from here on (the window's waker).
    fn set_notifier(&self, notify: Notifier);

    /// Adds `server`, starts connecting it and saves it. Err: why it can't be added, before
    /// anything changes. Saving happens in the background; see [`take_problem`](Self::take_problem).
    fn add(&self, server: NewServer) -> Result<(), String> {
        let _ = server;
        Err("servers can't be added here".into())
    }

    /// Removes the server `name` (closing it) and saves that.
    fn remove(&self, name: &str) -> Result<(), String> {
        Err(format!("`{name}` can't be removed here"))
    }

    /// Starts (and connects) or stops the server `name`, and saves that.
    fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        let _ = enabled;
        Err(format!("`{name}` can't be changed here"))
    }

    /// Why the last change couldn't be saved, once (the version moves when it happens).
    fn take_problem(&self) -> Option<String> {
        None
    }
}

/// A fixed list, for scenes and tests.
#[derive(Default)]
pub struct FixedServers {
    statuses: Vec<ServerStatus>,
    file: Option<PathBuf>,
    notify: Mutex<Option<Notifier>>,
}

impl FixedServers {
    pub fn new(statuses: Vec<ServerStatus>, file: Option<PathBuf>) -> Self {
        FixedServers { statuses, file, notify: Mutex::new(None) }
    }

    /// Whether a notifier was handed over (tests).
    pub fn has_notifier(&self) -> bool {
        self.notify.lock().unwrap_or_else(PoisonError::into_inner).is_some()
    }
}

impl ToolServers for FixedServers {
    fn version(&self) -> u64 {
        0
    }

    fn statuses(&self) -> Vec<ServerStatus> {
        self.statuses.clone()
    }

    fn config_file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    fn set_notifier(&self, notify: Notifier) {
        *self.notify.lock().unwrap_or_else(PoisonError::into_inner) = Some(notify);
    }
}
