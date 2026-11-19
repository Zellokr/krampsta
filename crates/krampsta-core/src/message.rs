//! Messages in a conversation.

use crate::{MessageId, RunId, ToolCall, ToolResult};

/// Who wrote a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageRole {
    User,
    Assistant,
    /// Instructions for the model; not shown as a bubble.
    System,
    /// Output a tool returned to the model.
    Tool,
}

impl MessageRole {
    pub fn label(self) -> &'static str {
        match self {
            MessageRole::User => "You",
            MessageRole::Assistant => "Krampsta",
            MessageRole::System => "System",
            MessageRole::Tool => "Tool",
        }
    }
}

/// One message. Assistant messages grow while their run streams text into them.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: MessageId,
    pub role: MessageRole,
    pub content: String,
    /// What the model wrote while thinking, before (or between) its reply. Shown folded;
    /// never sent back to the provider.
    pub reasoning: String,
    /// The run that produced it (assistant and tool messages).
    pub run: Option<RunId>,
    /// Tool calls the assistant made while writing this message, in order.
    pub tool_calls: Vec<ToolCall>,
    /// Results for those calls, matched by [`ToolResult::call_id`].
    pub tool_results: Vec<ToolResult>,
}

impl Message {
    pub fn new(id: MessageId, role: MessageRole, content: impl Into<String>) -> Self {
        Message {
            id,
            role,
            content: content.into(),
            reasoning: String::new(),
            run: None,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
        }
    }

    pub fn user(id: MessageId, content: impl Into<String>) -> Self {
        Self::new(id, MessageRole::User, content)
    }

    pub fn assistant(id: MessageId, run: RunId) -> Self {
        Message { run: Some(run), ..Self::new(id, MessageRole::Assistant, "") }
    }

    /// The result recorded for a call, if it has finished.
    pub fn result_for(&self, call: &ToolCall) -> Option<&ToolResult> {
        self.tool_results.iter().find(|r| r.call_id == call.id)
    }
}
