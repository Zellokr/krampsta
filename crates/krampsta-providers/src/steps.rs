//! What every engine's agent loop shares: a request, the tool calls it asked for run, their
//! results back to the model, again — until a reply asks for none.

use futures_util::future::try_join_all;
use krampsta_agent::{AgentError, RunContext};
use krampsta_core::{ToolCall, ToolCallId, ToolResult};

/// Added to Krampsta's instructions when the run has tools.
pub const TOOLS_HINT: &str = "You work in the user's project folder. Before answering about their code, look \
at it with the tools: list_files to see the layout, search to find where something is, read_file to read it. \
Paths are relative to the project root. Prefer a few targeted searches over reading whole trees.";

/// Requests one run may make before Krampsta stops it: a model calling tools in circles would
/// otherwise spend tokens forever.
pub const MAX_STEPS: usize = 50;

pub fn too_many_steps() -> AgentError {
    AgentError::Provider(format!(
        "the model kept calling tools for {MAX_STEPS} steps without answering, so Krampsta stopped it"
    ))
}

/// Runs one step's calls at the same time (they only read), their results in the calls' order.
pub async fn run_calls(ctx: &RunContext, calls: &[ToolCall]) -> Result<Vec<ToolResult>, AgentError> {
    try_join_all(calls.iter().cloned().map(|call| ctx.run_tool(call))).await
}

/// A call id for providers that send none, unique within the run.
pub fn call_id(step: usize, index: usize) -> ToolCallId {
    ToolCallId(format!("call_{step}_{index}"))
}

/// The reply streams into one message across steps; text that resumes after tool calls starts a
/// paragraph of its own instead of running on from the last word.
#[derive(Default)]
pub struct Paragraphs {
    wrote: bool,
    gap: bool,
}

impl Paragraphs {
    pub fn text(&mut self, ctx: &RunContext, text: &str) {
        if self.gap {
            ctx.text("\n\n");
            self.gap = false;
        }
        self.wrote = true;
        ctx.text(text);
    }

    /// A step ended in tool calls: the next text, if any, is a new paragraph.
    pub fn next_step(&mut self) {
        self.gap = self.wrote;
    }
}
