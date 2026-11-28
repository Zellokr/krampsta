//! A session as it is written to disk: one JSON document per session.
//!
//! The on-disk shape is its own set of types, so the domain model can change without breaking
//! saved history, and what only matters while a run is live (the run it accepts events from, a
//! running command's output) is never written.

use krampsta_core::{
    AgentStatus, Message, MessageId, MessageRole, ModelUsage, RunId, Session, SessionId, ToolCall, ToolCallId,
    ToolResult, Usage,
};
use serde::{Deserialize, Serialize};

/// Bumped when the shape changes in a way older builds can't read. 2: the `summary` role.
pub const VERSION: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedSession {
    pub version: u32,
    pub id: u64,
    pub title: String,
    /// The project folder; absent in sessions saved before projects (they take the open one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<std::path::PathBuf>,
    pub status: SavedStatus,
    /// Yolo mode; absent when off, and in sessions saved before it existed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub yolo: bool,
    #[serde(default)]
    pub usage: SavedUsage,
    /// The same tokens by model; absent in sessions saved before it was kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_model: Vec<SavedModelUsage>,
    pub messages: Vec<SavedMessage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedModelUsage {
    pub provider: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
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
    Summary,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedMessage {
    pub id: u64,
    pub role: SavedRole,
    pub content: String,
    /// The file in `images/` that holds its image (`images.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Its image as base64, as the first builds with images wrote it into the JSON; read, never
    /// written (the next save moves it to a file). Also where `read_session` puts the file's.
    #[serde(default, skip_serializing)]
    pub image_png: Option<String>,
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
            yolo: s.yolo,
            status: match &s.status {
                AgentStatus::Idle => SavedStatus::Idle,
                AgentStatus::Running => SavedStatus::Running,
                AgentStatus::Completed => SavedStatus::Completed,
                AgentStatus::Failed { error } => SavedStatus::Failed { error: error.clone() },
                AgentStatus::Cancelled => SavedStatus::Cancelled,
            },
            usage: SavedUsage { input: s.usage.input, output: s.usage.output },
            by_model: s
                .by_model
                .iter()
                .map(|m| SavedModelUsage {
                    provider: m.provider.clone(),
                    model: m.model.clone(),
                    input: m.usage.input,
                    output: m.usage.output,
                })
                .collect(),
            messages: s.messages.iter().map(|m| SavedMessage::from_message(s.id, m)).collect(),
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
        session.yolo = self.yolo;
        session.by_model = self
            .by_model
            .into_iter()
            .map(|m| ModelUsage {
                provider: m.provider,
                model: m.model,
                usage: Usage { input: m.input, output: m.output },
            })
            .collect();
        session
    }
}

impl SavedMessage {
    fn from_message(session: SessionId, m: &Message) -> Self {
        SavedMessage {
            id: m.id.0,
            role: match m.role {
                MessageRole::User => SavedRole::User,
                MessageRole::Assistant => SavedRole::Assistant,
                MessageRole::System => SavedRole::System,
                MessageRole::Tool => SavedRole::Tool,
                MessageRole::Summary => SavedRole::Summary,
            },
            content: m.content.clone(),
            image: m.image_png.as_ref().map(|_| crate::images::name(session, m.id)),
            image_png: None,
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
            SavedRole::Summary => MessageRole::Summary,
        };
        let mut message = Message::new(MessageId(self.id), role, self.content);
        message.image_png = self.image_png.map(Into::into);
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
