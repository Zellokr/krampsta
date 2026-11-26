//! Messages in a conversation.

use crate::{MessageId, RunId, ToolCall, ToolCallId, ToolResult};

/// Who wrote a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageRole {
    User,
    Assistant,
    /// Instructions for the model; not shown as a bubble.
    System,
    /// Output a tool returned to the model.
    Tool,
    /// What the conversation before it said, summed up by the model to fit its context: from
    /// here on the model is sent this instead of the messages it covers (the user still sees them).
    Summary,
}

impl MessageRole {
    pub fn label(self) -> &'static str {
        match self {
            MessageRole::User => "You",
            MessageRole::Assistant => "Krampsta",
            MessageRole::System => "System",
            MessageRole::Tool => "Tool",
            MessageRole::Summary => "Summary",
        }
    }
}

/// One message. Assistant messages grow while their run streams text into them.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: MessageId,
    pub role: MessageRole,
    pub content: String,
    /// Optional PNG attachment, encoded as standard base64.
    pub image_png: Option<std::sync::Arc<str>>,
    /// What the model wrote while thinking, before (or between) its reply. Shown folded;
    /// never sent back to the provider.
    pub reasoning: String,
    /// The run that produced it (assistant and tool messages).
    pub run: Option<RunId>,
    /// Tool calls the assistant made while writing this message, in order.
    pub tool_calls: Vec<ToolCall>,
    /// Results for those calls, matched by [`ToolResult::call_id`].
    pub tool_results: Vec<ToolResult>,
    /// Output of calls still running (a command's), shown until their result replaces it. Only
    /// its last [`LIVE_OUTPUT_MAX`] bytes are kept.
    pub live_output: Vec<(ToolCallId, String)>,
}

/// The most live output kept per call: its tail is what shows a command's progress.
pub const LIVE_OUTPUT_MAX: usize = 32 * 1024;

impl Message {
    pub fn new(id: MessageId, role: MessageRole, content: impl Into<String>) -> Self {
        Message {
            id,
            role,
            content: content.into(),
            image_png: None,
            reasoning: String::new(),
            run: None,
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            live_output: Vec::new(),
        }
    }

    /// The output a running call has shown so far.
    pub fn live_output_for(&self, call: &ToolCall) -> Option<&str> {
        self.live_output.iter().find(|(id, _)| *id == call.id).map(|(_, text)| text.as_str())
    }

    /// Appends to a running call's output, dropping its oldest bytes past the cap.
    pub(crate) fn push_live_output(&mut self, call: &ToolCallId, text: &str) {
        let at = match self.live_output.iter().position(|(id, _)| id == call) {
            Some(at) => at,
            None => {
                self.live_output.push((call.clone(), String::new()));
                self.live_output.len() - 1
            }
        };
        let out = &mut self.live_output[at].1;
        out.push_str(text);
        if out.len() > LIVE_OUTPUT_MAX {
            // Cut at a line (or at least a character) boundary near the cap.
            let mut cut = out.len() - LIVE_OUTPUT_MAX;
            while !out.is_char_boundary(cut) {
                cut += 1;
            }
            let cut = out[cut..].find('\n').map_or(cut, |n| cut + n + 1);
            out.drain(..cut);
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
