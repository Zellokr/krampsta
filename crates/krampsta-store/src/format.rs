//! A session as it is written to disk: one JSON document per session.
//!
//! The on-disk shape is its own set of types, so the domain model can change without breaking
//! saved history, and what only matters while a run is live (the run it accepts events from, a
//! running command's output) is never written.

use krampsta_core::{
    AgentStatus, Message, MessageId, MessageRole, RunId, Session, SessionId, ToolCall, ToolCallId, ToolResult, Usage,
};
use serde::{Deserialize, Serialize};

/// Bumped when the shape changes in a way older builds can't read.
pub const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedSession {
    pub version: u32,
    pub id: u64,
    pub title: String,
    /// The project folder; absent in sessions saved before projects (they take the open one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<std::path::PathBuf>,
    pub status: SavedStatus,
    #[serde(default)]
    pub usage: SavedUsage,
    pub messages: Vec<SavedMessage>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedStatus {
    Idle,
    Running,
    Completed,
    Failed { error: String },
    Cancelled,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SavedUsage {
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedMessage {
    pub id: u64,
    pub role: SavedRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<SavedCall>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<SavedResult>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedResult {
    pub call_id: String,
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
}

impl SavedSession {
    pub fn from_session(s: &Session) -> Self {
        SavedSession {
            version: VERSION,
            id: s.id.0,
            title: s.title.clone(),
            project: s.project.clone(),
            status: match &s.status {
                AgentStatus::Idle => SavedStatus::Idle,
                AgentStatus::Running => SavedStatus::Running,
                AgentStatus::Completed => SavedStatus::Completed,
                AgentStatus::Failed { error } => SavedStatus::Failed { error: error.clone() },
                AgentStatus::Cancelled => SavedStatus::Cancelled,
            },
            usage: SavedUsage { input: s.usage.input, output: s.usage.output },
            messages: s.messages.iter().map(SavedMessage::from_message).collect(),
        }
    }

    pub fn into_session(self) -> Session {
        let status = match self.status {
            SavedStatus::Idle => AgentStatus::Idle,
            SavedStatus::Running => AgentStatus::Running,
            SavedStatus::Completed => AgentStatus::Completed,
            SavedStatus::Failed { error } => AgentStatus::Failed { error },
            SavedStatus::Cancelled => AgentStatus::Cancelled,
        };
        let usage = Usage { input: self.usage.input, output: self.usage.output };
        let messages = self.messages.into_iter().map(SavedMessage::into_message).collect();
        let mut session = Session::restore(SessionId(self.id), self.title, messages, status, usage);
        session.project = self.project;
        session
    }
}

impl SavedMessage {
    fn from_message(m: &Message) -> Self {
        SavedMessage {
            id: m.id.0,
            role: match m.role {
                MessageRole::User => SavedRole::User,
                MessageRole::Assistant => SavedRole::Assistant,
                MessageRole::System => SavedRole::System,
                MessageRole::Tool => SavedRole::Tool,
            },
            content: m.content.clone(),
            reasoning: m.reasoning.clone(),
            run: m.run.map(|r| r.0),
            tool_calls: m
                .tool_calls
                .iter()
                .map(|c| SavedCall { id: c.id.0.clone(), name: c.name.clone(), arguments: c.arguments.clone() })
                .collect(),
            tool_results: m
                .tool_results
                .iter()
                .map(|r| SavedResult { call_id: r.call_id.0.clone(), output: r.output.clone(), is_error: r.is_error })
                .collect(),
        }
    }

    fn into_message(self) -> Message {
        let role = match self.role {
            SavedRole::User => MessageRole::User,
            SavedRole::Assistant => MessageRole::Assistant,
            SavedRole::System => MessageRole::System,
            SavedRole::Tool => MessageRole::Tool,
        };
        let mut message = Message::new(MessageId(self.id), role, self.content);
        message.reasoning = self.reasoning;
        message.run = self.run.map(RunId);
        message.tool_calls = self
            .tool_calls
            .into_iter()
            .map(|c| ToolCall { id: ToolCallId(c.id), name: c.name, arguments: c.arguments })
            .collect();
        message.tool_results = self
            .tool_results
            .into_iter()
            .map(|r| ToolResult { call_id: ToolCallId(r.call_id), output: r.output, is_error: r.is_error })
            .collect();
        message
    }
}
