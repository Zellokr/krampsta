//! A conversation with the agent and the state machine that folds run events into it.

use std::path::PathBuf;

use crate::{AgentEvent, AgentStatus, Message, MessageId, MessageRole, RunId, SessionId, Usage};

/// The title a session has until its first message names it.
pub const UNTITLED: &str = "New session";
const TITLE_MAX_CHARS: usize = 48;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("the message is empty")]
    EmptyMessage,
    #[error("the agent is already running in this session")]
    AlreadyRunning,
}

/// A conversation: its messages and the status of its agent.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: SessionId,
    pub title: String,
    pub messages: Vec<Message>,
    pub status: AgentStatus,
    /// The project folder it works in. None: not tied to one yet (it takes the project open
    /// when it next runs).
    pub project: Option<PathBuf>,
    /// Tokens every run in this session has used so far, as the providers counted them.
    pub usage: Usage,
    /// The same tokens by the model that used them (what a price applies to). Runs that
    /// weren't billed to a model ([`Session::bill_to`]) count only in `usage`.
    pub by_model: Vec<ModelUsage>,
    /// The run whose events this session accepts. Events from any other run are stale
    /// (a cancelled run that kept talking, say) and change nothing.
    active_run: Option<RunId>,
    /// Which model the active run's tokens are counted against.
    billed: Option<(String, String)>,
    next_message: u64,
}

/// Tokens one model used in a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelUsage {
    pub provider: String,
    pub model: String,
    pub usage: Usage,
}

impl Session {
    pub fn new(id: SessionId) -> Self {
        Self::with_title(id, UNTITLED)
    }

    pub fn with_title(id: SessionId, title: impl Into<String>) -> Self {
        Session {
            id,
            title: title.into(),
            messages: Vec::new(),
            status: AgentStatus::Idle,
            usage: Usage::default(),
            by_model: Vec::new(),
            project: None,
            active_run: None,
            billed: None,
            next_message: 1,
        }
    }

    /// A session as it was saved. No run is attached: one that was running when saved is
    /// marked [`AgentStatus::Cancelled`], since nothing will finish it. New messages are
    /// numbered after the highest id in `messages`.
    pub fn restore(id: SessionId, title: String, messages: Vec<Message>, status: AgentStatus, usage: Usage) -> Self {
        let next_message = messages.iter().map(|m| m.id.0).max().map_or(1, |max| max.saturating_add(1));
        let status = if status.is_running() { AgentStatus::Cancelled } else { status };
        Session {
            id,
            title,
            messages,
            status,
            usage,
            by_model: Vec::new(),
            project: None,
            active_run: None,
            billed: None,
            next_message,
        }
    }

    pub fn active_run(&self) -> Option<RunId> {
        self.active_run
    }

    /// Counts the active run's tokens against `model` of `provider` (call it right after
    /// [`submit`](Self::submit)). The run's end forgets it.
    pub fn bill_to(&mut self, provider: &str, model: &str) {
        if self.active_run.is_some() {
            self.billed = Some((provider.to_owned(), model.to_owned()));
        }
    }

    fn add_usage(&mut self, usage: Usage) {
        self.usage += usage;
        let Some((provider, model)) = &self.billed else { return };
        match self.by_model.iter_mut().find(|m| m.provider == *provider && m.model == *model) {
            Some(entry) => entry.usage += usage,
            None => {
                self.by_model.push(ModelUsage { provider: provider.clone(), model: model.clone(), usage });
            }
        }
    }

    fn mint_message_id(&mut self) -> MessageId {
        let id = MessageId(self.next_message);
        self.next_message += 1;
        id
    }

    /// Adds a message as written, with no run attached (seeding a demo or restoring history).
    pub fn push_message(&mut self, role: MessageRole, content: impl Into<String>) -> MessageId {
        let id = self.mint_message_id();
        self.messages.push(Message::new(id, role, content));
        id
    }

    /// Adds the user's message and reserves the session for `run`.
    ///
    /// Fails if the text is blank or a run is already in progress; on failure nothing changes.
    pub fn submit(&mut self, text: &str, run: RunId) -> Result<MessageId, SessionError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(SessionError::EmptyMessage);
        }
        if self.status.is_running() {
            return Err(SessionError::AlreadyRunning);
        }
        if self.title == UNTITLED {
            self.title = title_from(text);
        }
        let id = self.mint_message_id();
        self.messages.push(Message::user(id, text));
        self.active_run = Some(run);
        self.status = AgentStatus::Running;
        Ok(id)
    }

    /// Folds one event into the session. Returns whether anything changed.
    pub fn apply(&mut self, event: &AgentEvent) -> bool {
        let run = event.run();
        if self.active_run != Some(run) {
            return false;
        }
        match event {
            AgentEvent::Started { .. } => {
                self.status = AgentStatus::Running;
                self.assistant_message(run);
            }
            AgentEvent::TextDelta { text, .. } => self.assistant_message(run).content.push_str(text),
            AgentEvent::ReasoningDelta { text, .. } => self.assistant_message(run).reasoning.push_str(text),
            AgentEvent::ToolCallStarted { call, .. } => self.assistant_message(run).tool_calls.push(call.clone()),
            AgentEvent::ToolCallFinished { result, .. } => {
                let message = self.assistant_message(run);
                // The result replaces what the call showed while running.
                message.live_output.retain(|(id, _)| *id != result.call_id);
                message.tool_results.push(result.clone());
            }
            AgentEvent::ToolOutputDelta { call, text, .. } => self.assistant_message(run).push_live_output(call, text),
            // The app asks the user; the conversation shows the call as running meanwhile.
            AgentEvent::ApprovalRequested { .. } => return false,
            AgentEvent::Usage { usage, .. } => self.add_usage(*usage),
            // Changes belong to the project, not the conversation: the app keeps them.
            AgentEvent::ChangeProposed { .. } => return false,
            AgentEvent::Completed { .. } => self.finish(AgentStatus::Completed),
            AgentEvent::Failed { error, .. } => self.finish(AgentStatus::Failed { error: error.clone() }),
            AgentEvent::Cancelled { .. } => self.finish(AgentStatus::Cancelled),
        }
        true
    }

    fn finish(&mut self, status: AgentStatus) {
        self.status = status;
        self.active_run = None;
        self.billed = None;
        // A run that ended before writing (or thinking) anything leaves no empty bubble behind.
        self.messages.retain(|m| {
            !(m.role == MessageRole::Assistant
                && m.content.is_empty()
                && m.reasoning.is_empty()
                && m.tool_calls.is_empty())
        });
    }

    /// The assistant message `run` writes into, created on first use.
    fn assistant_message(&mut self, run: RunId) -> &mut Message {
        let exists = self.messages.last().is_some_and(|m| m.role == MessageRole::Assistant && m.run == Some(run));
        if !exists {
            let id = self.mint_message_id();
            self.messages.push(Message::assistant(id, run));
        }
        let last = self.messages.len() - 1;
        &mut self.messages[last]
    }

    /// The last line worth showing under the title in a list.
    pub fn preview(&self) -> Option<&str> {
        self.messages.iter().rev().find(|m| !m.content.trim().is_empty()).and_then(|m| m.content.lines().next())
    }
}

fn title_from(text: &str) -> String {
    let line = text.lines().next().unwrap_or(text).trim();
    if line.chars().count() <= TITLE_MAX_CHARS {
        return line.to_owned();
    }
    let mut title: String = line.chars().take(TITLE_MAX_CHARS - 1).collect();
    title.truncate(title.trim_end().len());
    title.push('…');
    title
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolCall, ToolCallId, ToolResult};

    fn session() -> Session {
        Session::new(SessionId(1))
    }

    #[test]
    fn submit_adds_the_message_names_the_session_and_runs() {
        let mut s = session();
        let id = s.submit("  Explain the borrow checker  ", RunId(1)).unwrap();
        assert_eq!(s.messages.len(), 1);
        assert_eq!(s.messages[0].id, id);
        assert_eq!(s.messages[0].content, "Explain the borrow checker");
        assert_eq!(s.title, "Explain the borrow checker");
        assert_eq!(s.status, AgentStatus::Running);
        assert_eq!(s.active_run(), Some(RunId(1)));
    }

    #[test]
    fn submit_rejects_blank_text_and_concurrent_runs() {
        let mut s = session();
        assert_eq!(s.submit("   \n", RunId(1)), Err(SessionError::EmptyMessage));
        assert!(s.messages.is_empty());
        s.submit("first", RunId(1)).unwrap();
        assert_eq!(s.submit("second", RunId(2)), Err(SessionError::AlreadyRunning));
        assert_eq!(s.messages.len(), 1);
    }

    #[test]
    fn long_first_messages_make_a_truncated_title() {
        let mut s = session();
        s.submit(&"word ".repeat(40), RunId(1)).unwrap();
        assert!(s.title.ends_with('…'));
        assert!(s.title.chars().count() <= TITLE_MAX_CHARS);
    }

    #[test]
    fn a_run_streams_into_one_assistant_message_and_completes() {
        let mut s = session();
        let run = RunId(4);
        s.submit("hi", run).unwrap();
        assert!(s.apply(&AgentEvent::Started { run }));
        let call = ToolCall { id: ToolCallId("c1".into()), name: "list_files".into(), arguments: "{}".into() };
        s.apply(&AgentEvent::ToolCallStarted { run, call: call.clone() });
        s.apply(&AgentEvent::ToolCallFinished {
            run,
            result: ToolResult { call_id: call.id.clone(), output: "src/".into(), is_error: false },
        });
        s.apply(&AgentEvent::TextDelta { run, text: "Hello ".into() });
        s.apply(&AgentEvent::TextDelta { run, text: "there".into() });
        s.apply(&AgentEvent::Completed { run });

        assert_eq!(s.messages.len(), 2);
        let reply = &s.messages[1];
        assert_eq!(reply.role, MessageRole::Assistant);
        assert_eq!(reply.content, "Hello there");
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.result_for(&call).map(|r| r.output.as_str()), Some("src/"));
        assert_eq!(s.status, AgentStatus::Completed);
        assert_eq!(s.active_run(), None);
    }

    #[test]
    fn reasoning_streams_apart_from_the_reply_and_keeps_a_textless_message() {
        let mut s = session();
        let run = RunId(3);
        s.submit("why?", run).unwrap();
        s.apply(&AgentEvent::Started { run });
        s.apply(&AgentEvent::ReasoningDelta { run, text: "Weighing ".into() });
        s.apply(&AgentEvent::ReasoningDelta { run, text: "options".into() });
        s.apply(&AgentEvent::TextDelta { run, text: "Because.".into() });
        let reply = s.messages.last().unwrap();
        assert_eq!(reply.reasoning, "Weighing options");
        assert_eq!(reply.content, "Because.");
        s.apply(&AgentEvent::Completed { run });

        // A run cancelled while still thinking keeps what it thought.
        let run = RunId(4);
        s.submit("and?", run).unwrap();
        s.apply(&AgentEvent::Started { run });
        s.apply(&AgentEvent::ReasoningDelta { run, text: "Hmm".into() });
        s.apply(&AgentEvent::Cancelled { run });
        assert_eq!(s.messages.last().unwrap().reasoning, "Hmm");
    }

    #[test]
    fn failures_record_the_error_and_drop_the_empty_reply() {
        let mut s = session();
        let run = RunId(2);
        s.submit("break", run).unwrap();
        s.apply(&AgentEvent::Started { run });
        s.apply(&AgentEvent::Failed { run, error: "boom".into() });
        assert_eq!(s.status, AgentStatus::Failed { error: "boom".into() });
        assert_eq!(s.messages.len(), 1, "no empty assistant bubble");
        // A new message can be sent after a failure.
        assert!(s.submit("again", RunId(3)).is_ok());
    }

    #[test]
    fn usage_adds_up_across_requests_and_runs_but_not_from_stale_ones() {
        let mut s = session();
        s.submit("one", RunId(1)).unwrap();
        s.apply(&AgentEvent::Usage { run: RunId(1), usage: Usage { input: 100, output: 20 } });
        s.apply(&AgentEvent::Usage { run: RunId(1), usage: Usage { input: 130, output: 5 } });
        s.apply(&AgentEvent::Cancelled { run: RunId(1) });
        assert!(!s.apply(&AgentEvent::Usage { run: RunId(1), usage: Usage { input: 9, output: 9 } }));
        s.submit("two", RunId(2)).unwrap();
        s.apply(&AgentEvent::Usage { run: RunId(2), usage: Usage { input: 170, output: 30 } });
        assert_eq!(s.usage, Usage { input: 400, output: 55 });
    }

    #[test]
    fn usage_is_also_counted_against_the_model_each_run_was_billed_to() {
        let mut s = session();
        s.submit("one", RunId(1)).unwrap();
        s.bill_to("openai", "gpt-a");
        s.apply(&AgentEvent::Usage { run: RunId(1), usage: Usage { input: 100, output: 10 } });
        s.apply(&AgentEvent::Completed { run: RunId(1) });
        s.submit("two", RunId(2)).unwrap();
        s.bill_to("openai", "gpt-b");
        s.apply(&AgentEvent::Usage { run: RunId(2), usage: Usage { input: 50, output: 5 } });
        s.apply(&AgentEvent::Completed { run: RunId(2) });
        s.submit("three", RunId(3)).unwrap();
        s.bill_to("openai", "gpt-a");
        s.apply(&AgentEvent::Usage { run: RunId(3), usage: Usage { input: 1, output: 1 } });
        s.apply(&AgentEvent::Cancelled { run: RunId(3) });
        // Not billed: counts in the total only.
        s.submit("four", RunId(4)).unwrap();
        s.apply(&AgentEvent::Usage { run: RunId(4), usage: Usage { input: 7, output: 7 } });

        assert_eq!(s.usage, Usage { input: 158, output: 23 });
        let by: Vec<(&str, Usage)> = s.by_model.iter().map(|m| (m.model.as_str(), m.usage)).collect();
        assert_eq!(by, [("gpt-a", Usage { input: 101, output: 11 }), ("gpt-b", Usage { input: 50, output: 5 })]);
        // Billing outside a run does nothing.
        s.apply(&AgentEvent::Completed { run: RunId(4) });
        s.bill_to("openai", "gpt-c");
        assert!(s.billed.is_none());
    }

    #[test]
    fn live_output_grows_keeps_its_tail_and_gives_way_to_the_result() {
        let mut s = session();
        let run = RunId(1);
        s.submit("build", run).unwrap();
        let call = ToolCall { id: ToolCallId("c".into()), name: "run_command".into(), arguments: "{}".into() };
        s.apply(&AgentEvent::ToolCallStarted { run, call: call.clone() });
        s.apply(&AgentEvent::ToolOutputDelta { run, call: call.id.clone(), text: "Compiling a\n".into() });
        s.apply(&AgentEvent::ToolOutputDelta { run, call: call.id.clone(), text: "Compiling b\n".into() });
        assert_eq!(s.messages[1].live_output_for(&call), Some("Compiling a\nCompiling b\n"));

        let long = "line\n".repeat(crate::message::LIVE_OUTPUT_MAX / 4);
        s.apply(&AgentEvent::ToolOutputDelta { run, call: call.id.clone(), text: long });
        let kept = s.messages[1].live_output_for(&call).unwrap();
        assert!(
            kept.len() <= crate::message::LIVE_OUTPUT_MAX && kept.starts_with("line\n"),
            "the tail, from a line start"
        );

        s.apply(&AgentEvent::ToolCallFinished {
            run,
            result: ToolResult { call_id: call.id.clone(), output: "done".into(), is_error: false },
        });
        assert_eq!(s.messages[1].live_output_for(&call), None);
    }

    #[test]
    fn events_from_stale_runs_change_nothing() {
        let mut s = session();
        s.submit("one", RunId(1)).unwrap();
        s.apply(&AgentEvent::Cancelled { run: RunId(1) });
        assert_eq!(s.status, AgentStatus::Cancelled);
        let before = s.clone();
        assert!(!s.apply(&AgentEvent::TextDelta { run: RunId(1), text: "late".into() }));
        assert!(!s.apply(&AgentEvent::Completed { run: RunId(1) }));
        assert_eq!(s, before);
    }

    #[test]
    fn restored_sessions_number_after_their_messages_and_never_resume_a_run() {
        let messages = vec![Message::user(MessageId(3), "hi"), Message::assistant(MessageId(7), RunId(2))];
        let mut s = Session::restore(SessionId(5), "Saved".into(), messages, AgentStatus::Running, Usage::default());
        assert_eq!(s.status, AgentStatus::Cancelled);
        assert_eq!(s.active_run(), None);
        assert!(!s.apply(&AgentEvent::TextDelta { run: RunId(2), text: "late".into() }));
        assert_eq!(s.push_message(MessageRole::User, "again"), MessageId(8));
    }

    #[test]
    fn preview_is_the_last_non_empty_line() {
        let mut s = session();
        assert_eq!(s.preview(), None);
        s.push_message(MessageRole::User, "first line\nsecond");
        assert_eq!(s.preview(), Some("first line"));
    }
}
