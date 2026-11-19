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

/// Runs tools. Calls block (file I/O): the host runs them on a blocking thread, never on the
/// async workers or the UI thread, and cancels them through `cancel`, which long calls check.
pub trait Toolbox: Send + Sync + 'static {
    /// Every tool it offers, built once.
    fn specs(&self) -> Arc<[ToolSpec]>;

    /// Runs `name` with its JSON `arguments`. `Err` is a readable reason, which the model also
    /// gets (marked as an error) so it can try otherwise.
    fn run(&self, name: &str, arguments: &str, cancel: &CancellationToken) -> Result<ToolOutput, String>;

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
