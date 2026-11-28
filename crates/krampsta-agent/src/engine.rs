//! The engine contract.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use krampsta_core::{AgentEvent, Message, RunEvent, RunId, SessionId, ToolCall, ToolCallId, ToolResult, Usage};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::host::Notifier;
use crate::tools::{Access, Approval, Call, Decision, ToolOutput, ToolSpec, Toolbox};

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
    tools: Option<Arc<dyn Toolbox>>,
    specs: Arc<[ToolSpec]>,
    approvals: Approvals,
}

/// Calls waiting for the user, by run and call, each with the channel its answer goes down.
pub(crate) type Approvals = Arc<Mutex<HashMap<(RunId, ToolCallId), oneshot::Sender<Decision>>>>;

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every change is one insert or remove: a panic can't leave the map half-changed.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl RunContext {
    pub(crate) fn new(
        session: SessionId,
        run: RunId,
        events: Sender<RunEvent>,
        notify: Notifier,
        cancel: CancellationToken,
        tools: Option<Arc<dyn Toolbox>>,
        approvals: Approvals,
    ) -> Self {
        let specs = tools.as_ref().map_or_else(|| Arc::from([]), |t| t.specs());
        RunContext { session, run, events, notify, cancel, tools, specs, approvals }
    }

    /// A context with no audience and no tools, for engine tests that drive `run` by hand.
    pub fn detached(tools: Option<Arc<dyn Toolbox>>) -> (Self, std::sync::mpsc::Receiver<RunEvent>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = Self::new(
            SessionId(0),
            RunId(0),
            tx,
            Arc::new(|| {}),
            CancellationToken::new(),
            tools,
            Approvals::default(),
        );
        (ctx, rx)
    }

    /// The tools this run may offer the model; empty when it has none.
    pub fn tool_specs(&self) -> &[ToolSpec] {
        &self.specs
    }

    /// What the project tells the model ([`Toolbox::guidance`]), for the run's instructions;
    /// `None` without tools or when the project says nothing.
    pub fn guidance(&self) -> Option<Arc<str>> {
        self.tools.as_ref().and_then(|t| t.guidance())
    }

    /// Runs one tool call the model made: shows it starting, runs it on a blocking thread and
    /// shows its result, which goes back to the model. `Err` only when the run is cancelled
    /// (the card then shows it was stopped).
    pub async fn run_tool(&self, call: ToolCall) -> Result<ToolResult, AgentError> {
        self.tool_started(call.clone());
        let outcome = match self.tools.clone() {
            None => Err(format!("there is no tool named `{}`", call.name)),
            Some(tools) => match self.permit(&tools, &call).await {
                Err(AgentError::Cancelled) => return Err(self.stopped(call)),
                Err(e) => Err(e.to_string()),
                Ok(Err(refused)) => Err(refused),
                Ok(Ok(approval)) => {
                    let ctx = self.clone();
                    let (name, arguments, id) = (call.name.clone(), call.arguments.clone(), call.id.clone());
                    let job = tokio::task::spawn_blocking(move || {
                        let output = |text: &str| {
                            let text = text.to_owned();
                            ctx.emit(AgentEvent::ToolOutputDelta { run: ctx.run, call: id.clone(), text });
                        };
                        let call = Call { cancel: &ctx.cancel, approval: &approval, output: &output };
                        tools.run(&name, &arguments, &call)
                    });
                    tokio::select! {
                        biased;
                        () = self.cancel.cancelled() => return Err(self.stopped(call)),
                        joined = job => joined.unwrap_or_else(|e| Err(format!("the tool stopped abnormally: {e}"))),
                    }
                }
            },
        };
        let (output, is_error) = match outcome {
            Ok(ToolOutput { text, change }) => {
                if let Some(change) = change {
                    self.emit(AgentEvent::ChangeProposed { run: self.run, call: call.id.clone(), change });
                }
                (text, false)
            }
            Err(reason) => (reason, true),
        };
        let result = ToolResult { call_id: call.id, output, is_error };
        self.tool_finished(result.clone());
        Ok(result)
    }

    /// Whether `call` may run: by a rule, or by the user, who is asked and waited for. The inner
    /// `Err` is why not (for the model); the outer one only means the run was cancelled.
    async fn permit(&self, tools: &Arc<dyn Toolbox>, call: &ToolCall) -> Result<Result<Approval, String>, AgentError> {
        let what = match tools.check(&call.name, &call.arguments) {
            Access::Allow(approval) => return Ok(Ok(approval)),
            Access::Deny(reason) => return Ok(Err(format!("Not allowed: {reason}"))),
            Access::Ask { what } => what,
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let key = (self.run, call.id.clone());
        lock(&self.approvals).insert(key.clone(), tx);
        self.emit(AgentEvent::ApprovalRequested {
            run: self.run,
            call: call.id.clone(),
            tool: call.name.clone(),
            what,
        });
        let answer = tokio::select! {
            biased;
            () = self.cancel.cancelled() => None,
            answer = rx => answer.ok(),
        };
        lock(&self.approvals).remove(&key);
        Ok(match answer {
            None => return Err(AgentError::Cancelled),
            Some(Decision::Deny) => {
                Err("The user did not allow this. Ask them how to go on, or do it another way.".into())
            }
            Some(Decision::Once) => Ok(Approval::Once),
            Some(Decision::Yolo) => Ok(Approval::Yolo),
            Some(Decision::Always) => {
                let (tools, name, arguments) = (tools.clone(), call.name.clone(), call.arguments.clone());
                // Allowed either way; only the rule for next time may fail to save.
                match tokio::task::spawn_blocking(move || tools.remember(&name, &arguments)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => self.text(format!("\n\n(Krampsta could not save the rule: {e})\n\n")),
                    Err(e) => self.text(format!("\n\n(Krampsta could not save the rule: {e})\n\n")),
                }
                Ok(Approval::Always)
            }
        })
    }

    /// Ends a call's card when the run is cancelled under it.
    fn stopped(&self, call: ToolCall) -> AgentError {
        self.tool_finished(ToolResult { call_id: call.id, output: "Stopped.".into(), is_error: true });
        AgentError::Cancelled
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
