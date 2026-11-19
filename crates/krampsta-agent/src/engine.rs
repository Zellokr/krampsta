//! The engine contract.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use krampsta_core::{AgentEvent, Message, RunEvent, RunId, SessionId, ToolCall, ToolResult, Usage};
use tokio_util::sync::CancellationToken;

use crate::host::Notifier;

/// A boxed, sendable future, so [`AgentEngine`] stays object safe (`Arc<dyn AgentEngine>`).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Why a run stopped early.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum AgentError {
    #[error("cancelled")]
    Cancelled,
    #[error("provider error: {0}")]
    Provider(String),
    #[error("tool `{tool}` failed: {message}")]
    Tool { tool: String, message: String },
    #[error("the agent runtime could not start: {0}")]
    Runtime(String),
}

/// Everything an engine needs to answer one user turn.
#[derive(Clone, Debug)]
pub struct RunRequest {
    pub session: SessionId,
    pub run: RunId,
    /// The conversation so far, ending with the user's new message.
    pub history: Vec<Message>,
    /// How hard a reasoning model should think; `None` leaves the model's default. Engines
    /// whose provider can't take it ignore it.
    pub effort: Option<Effort>,
}

/// How much a reasoning model thinks before answering, as the API spells it (`low`, `high`,
/// `xhigh`…). Open-ended: each model announces the levels it takes ([`EffortLevel`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Effort(String);

impl Effort {
    pub fn new(id: impl Into<String>) -> Self {
        Effort(id.into())
    }

    /// As the APIs spell it.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// As pickers show it: the known levels by name, others capitalised.
    pub fn label(&self) -> String {
        match self.0.as_str() {
            "none" => "Off".into(),
            "xhigh" => "Extra high".into(),
            other => {
                let mut chars = other.chars();
                chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
            }
        }
    }
}

/// A level a model takes, with what it's for ("Greater reasoning depth for complex problems").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffortLevel {
    pub effort: Effort,
    pub description: Option<String>,
}

impl EffortLevel {
    /// The levels to offer when a provider takes an effort but hasn't said which.
    pub fn defaults() -> Vec<EffortLevel> {
        ["low", "medium", "high"].map(|id| EffortLevel { effort: Effort::new(id), description: None }).into()
    }
}

impl RunRequest {
    /// The user's latest message, the one this run answers.
    pub fn prompt(&self) -> &str {
        self.history
            .iter()
            .rev()
            .find(|m| m.role == krampsta_core::MessageRole::User)
            .map_or("", |m| m.content.as_str())
    }
}

/// A run's link back to the host: emit events, watch for cancellation.
///
/// Engines emit only the middle of a run (text, tool calls). The host sends `Started` before
/// the engine runs and exactly one terminal event after it returns.
#[derive(Clone)]
pub struct RunContext {
    session: SessionId,
    run: RunId,
    events: Sender<RunEvent>,
    notify: Notifier,
    cancel: CancellationToken,
}

impl RunContext {
    pub(crate) fn new(
        session: SessionId,
        run: RunId,
        events: Sender<RunEvent>,
        notify: Notifier,
        cancel: CancellationToken,
    ) -> Self {
        RunContext { session, run, events, notify, cancel }
    }

    pub fn run(&self) -> RunId {
        self.run
    }

    pub fn session(&self) -> SessionId {
        self.session
    }

    /// The token cancelled when the user stops this run.
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancel
    }

    /// `Err(Cancelled)` once the run has been cancelled; engines call it between steps.
    pub fn check_cancelled(&self) -> Result<(), AgentError> {
        if self.cancel.is_cancelled() { Err(AgentError::Cancelled) } else { Ok(()) }
    }

    /// Sends an event to the UI and wakes it. A closed receiver (the UI went away) is not an
    /// error for the engine: the run simply has no audience.
    pub(crate) fn emit(&self, event: AgentEvent) {
        if self.events.send(RunEvent { session: self.session, event }).is_ok() {
            (self.notify)();
        }
    }

    /// Streams more of the reply.
    pub fn text(&self, text: impl Into<String>) {
        self.emit(AgentEvent::TextDelta { run: self.run, text: text.into() });
    }

    /// More of the model's reasoning, shown folded apart from the reply.
    pub fn reasoning(&self, text: impl Into<String>) {
        self.emit(AgentEvent::ReasoningDelta { run: self.run, text: text.into() });
    }

    /// The tokens a request used, once the provider reports them. Empty counts are dropped.
    pub fn usage(&self, usage: Usage) {
        if !usage.is_empty() {
            self.emit(AgentEvent::Usage { run: self.run, usage });
        }
    }

    pub fn tool_started(&self, call: ToolCall) {
        self.emit(AgentEvent::ToolCallStarted { run: self.run, call });
    }

    pub fn tool_finished(&self, result: ToolResult) {
        self.emit(AgentEvent::ToolCallFinished { run: self.run, result });
    }

    /// Waits `duration`, or returns `Err(Cancelled)` as soon as the run is cancelled.
    pub async fn sleep(&self, duration: std::time::Duration) -> Result<(), AgentError> {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(AgentError::Cancelled),
            () = tokio::time::sleep(duration) => Ok(()),
        }
    }
}

/// An agent: something that answers a [`RunRequest`] by emitting events through its context.
///
/// Implementations must be cheap to share (`Arc<dyn AgentEngine>`) and must not block the
/// thread they run on: do I/O with async APIs and wait with [`RunContext::sleep`] or another
/// cancellable future.
pub trait AgentEngine: Send + Sync + 'static {
    /// A short name for settings and logs ("mock", "anthropic", …).
    fn name(&self) -> &str;

    /// Runs one turn. Returning `Ok(())` completes the run, `Err` fails it
    /// (`Err(AgentError::Cancelled)` marks it cancelled).
    fn run(self: Arc<Self>, request: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>>;
}
