//! The tools an agent may call, as engines see them: their specs (sent to the provider) and a
//! [`Toolbox`] that runs them. The tools themselves live elsewhere (`krampsta-tools`).

use std::sync::Arc;

use krampsta_core::FileChange;
use tokio_util::sync::CancellationToken;

/// One tool, as a provider's tool-calling API describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    /// The name the model calls it by (`read_file`).
    pub name: String,
    /// What it does and when to use it, for the model.
    pub description: String,
    /// The JSON Schema of its arguments (an `object`).
    pub parameters: serde_json::Value,
}

/// What a call returns: the text for the model and, for a tool that edits, the change it
/// proposes (shown for review, never written by the tool).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub change: Option<FileChange>,
}

impl From<String> for ToolOutput {
    fn from(text: String) -> Self {
        ToolOutput { text, change: None }
    }
}

/// Whether a call may run, as the permission rules say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// It runs, permitted as `Approval` says.
    Allow(Approval),
    /// The user decides; `what` is what they are shown (the command line).
    Ask { what: String },
    /// A rule forbids it; the reason goes to the model.
    Deny(String),
}

/// How a call came to be permitted, for the record of what ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Approval {
    /// The tool needs no permission (it only reads, or only proposes).
    NotNeeded,
    /// An `allow` rule matched (the rule as written).
    Rule(String),
    /// The user allowed this call.
    Once,
    /// The user allowed it from now on.
    Always,
}

/// The user's answer to a call that asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Deny,
    Once,
    Always,
}

/// What a running call gets besides its arguments.
pub struct Call<'a> {
    /// Cancelled when the user stops the run; long calls check it and stop.
    pub cancel: &'a CancellationToken,
    pub approval: &'a Approval,
    /// Shows output as it is produced (a command's), before the call returns. Each piece is
    /// appended to what came before.
    pub output: &'a (dyn Fn(&str) + Send + Sync),
}

/// Runs tools. Calls block (file I/O, processes): the host runs them on a blocking thread, never
/// on the async workers or the UI thread.
pub trait Toolbox: Send + Sync + 'static {
    /// Every tool it offers, built once.
    fn specs(&self) -> Arc<[ToolSpec]>;

    /// Runs `name` with its JSON `arguments`. `Err` is a readable reason, which the model also
    /// gets (marked as an error) so it can try otherwise.
    fn run(&self, name: &str, arguments: &str, call: &Call<'_>) -> Result<ToolOutput, String>;

    /// Whether `name` may run with `arguments`, before it does. Cheap: no I/O.
    fn check(&self, name: &str, arguments: &str) -> Access {
        let _ = (name, arguments);
        Access::Allow(Approval::NotNeeded)
    }

    /// The user allowed this call from now on: keep a rule for it. May block (it saves a file).
    fn remember(&self, name: &str, arguments: &str) -> Result<(), String> {
        let _ = (name, arguments);
        Ok(())
    }

    /// Writes `content` to `path` (relative to the project) if the file still holds `expected`
    /// (`None`: it must not exist), so an accepted change never clobbers an edit made since.
    fn write(&self, path: &str, expected: Option<&str>, content: &str) -> Result<(), String> {
        let _ = (expected, content);
        Err(format!("`{path}` can't be written: these tools only read"))
    }

    /// What the agent's next edits and reads of `path` build on: the change still proposed for
    /// it, or the file on disk (`None`) once the change is accepted or rejected.
    fn set_pending(&self, path: &str, content: Option<Arc<str>>) {
        let _ = (path, content);
    }
}
