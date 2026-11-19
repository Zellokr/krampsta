//! The tools an agent may call, as engines see them: their specs (sent to the provider) and a
//! [`Toolbox`] that runs them. The tools themselves live elsewhere (`krampsta-tools`).

use std::sync::Arc;

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

/// Runs tools. Calls block (file I/O): the host runs them on a blocking thread, never on the
/// async workers, and cancels them through `cancel`, which long calls check between steps.
pub trait Toolbox: Send + Sync + 'static {
    /// Every tool it offers, built once.
    fn specs(&self) -> Arc<[ToolSpec]>;

    /// Runs `name` with its JSON `arguments`. `Ok` is the output for the model; `Err` is a
    /// readable reason, which the model also gets (marked as an error) so it can try otherwise.
    fn run(&self, name: &str, arguments: &str, cancel: &CancellationToken) -> Result<String, String>;
}
