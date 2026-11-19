//! What an agent run reports while it works.

use crate::{FileChange, RunId, SessionId, ToolCallId};

/// The state of a session's agent. A plain enum: each variant carries only what is valid in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AgentStatus {
    /// Nothing has run yet in this session.
    #[default]
    Idle,
    /// A run is in progress.
    Running,
    /// The last run finished normally.
    Completed,
    /// The last run stopped with an error.
    Failed { error: String },
    /// The last run was cancelled by the user.
    Cancelled,
}

impl AgentStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, AgentStatus::Running)
    }

    /// A short label for status indicators.
    pub fn label(&self) -> &'static str {
        match self {
            AgentStatus::Idle => "Idle",
            AgentStatus::Running => "Running",
            AgentStatus::Completed => "Completed",
            AgentStatus::Failed { .. } => "Failed",
            AgentStatus::Cancelled => "Cancelled",
        }
    }
}

/// A tool the model asked to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    pub id: ToolCallId,
    /// The tool's name, such as `read_file`.
    pub name: String,
    /// Arguments as the provider sent them (JSON text).
    pub arguments: String,
}

/// What a tool returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: ToolCallId,
    pub output: String,
    pub is_error: bool,
}

/// Tokens a provider counted: what it read (the prompt, history included) and what it wrote
/// (reply and reasoning).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
}

impl Usage {
    pub fn is_empty(self) -> bool {
        self.input == 0 && self.output == 0
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Usage) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
    }
}

/// One step of a run, in the order the engine produced it.
///
/// A run always starts with [`AgentEvent::Started`] and ends with exactly one of
/// [`AgentEvent::Completed`], [`AgentEvent::Failed`] or [`AgentEvent::Cancelled`]. In between,
/// an engine streams text and reports tool calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEvent {
    Started {
        run: RunId,
    },
    /// More of the assistant's reply (streaming). Appended to the run's assistant message.
    TextDelta {
        run: RunId,
        text: String,
    },
    /// More of the model's reasoning (streaming): what it thinks before answering, as the
    /// provider reports it (a summary, for OpenAI's reasoning models). Appended to the run's
    /// assistant message, apart from its reply.
    ReasoningDelta {
        run: RunId,
        text: String,
    },
    ToolCallStarted {
        run: RunId,
        call: ToolCall,
    },
    ToolCallFinished {
        run: RunId,
        result: ToolResult,
    },
    /// More output from a running call (a command's), before its result.
    ToolOutputDelta {
        run: RunId,
        call: ToolCallId,
        text: String,
    },
    /// A call waits for the user to allow it: `what` is what they decide on (a command line).
    ApprovalRequested {
        run: RunId,
        call: ToolCallId,
        tool: String,
        what: String,
    },
    /// A tool proposed a change to a file (not written: the user reviews it).
    ChangeProposed {
        run: RunId,
        change: FileChange,
    },
    /// Tokens one request to the provider used. A run that makes several requests reports each.
    Usage {
        run: RunId,
        usage: Usage,
    },
    Completed {
        run: RunId,
    },
    Failed {
        run: RunId,
        error: String,
    },
    Cancelled {
        run: RunId,
    },
}

impl AgentEvent {
    pub fn run(&self) -> RunId {
        match self {
            AgentEvent::Started { run }
            | AgentEvent::TextDelta { run, .. }
            | AgentEvent::ReasoningDelta { run, .. }
            | AgentEvent::ToolCallStarted { run, .. }
            | AgentEvent::ToolCallFinished { run, .. }
            | AgentEvent::Usage { run, .. }
            | AgentEvent::ChangeProposed { run, .. }
            | AgentEvent::ToolOutputDelta { run, .. }
            | AgentEvent::ApprovalRequested { run, .. }
            | AgentEvent::Completed { run }
            | AgentEvent::Failed { run, .. }
            | AgentEvent::Cancelled { run } => *run,
        }
    }

    /// Whether this event ends its run.
    pub fn is_terminal(&self) -> bool {
        matches!(self, AgentEvent::Completed { .. } | AgentEvent::Failed { .. } | AgentEvent::Cancelled { .. })
    }
}

/// An [`AgentEvent`] addressed to the session whose run produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunEvent {
    pub session: SessionId,
    pub event: AgentEvent,
}
