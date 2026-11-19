//! An engine for OpenAI's Responses API (`POST {base}/responses`, streamed), which is what the
//! ChatGPT plan's backend speaks. Events arrive as `data: {"type": "response.…", …}`.

use std::sync::Arc;

use krampsta_agent::{AgentEngine, AgentError, BoxFuture, RunContext, RunRequest};
use krampsta_core::{Message, MessageRole};
use serde_json::{Value, json};

use crate::access::Access;
use crate::http::{self, Flow};
use crate::sse::SseEvent;

/// Krampsta's standing instructions to the model (the API requires some).
pub const INSTRUCTIONS: &str = "You are Krampsta, a coding assistant in a desktop app. Answer the user's \
questions about their code clearly and concisely, using Markdown and fenced code blocks.";

pub struct ResponsesEngine {
    access: Arc<dyn Access>,
    model: String,
    client: reqwest::Client,
    name: String,
}

impl ResponsesEngine {
    pub fn new(access: Arc<dyn Access>, model: &str, client: reqwest::Client, label: &str) -> Self {
        ResponsesEngine { access, model: model.to_owned(), client, name: format!("{label} · {model}") }
    }
}

impl AgentEngine for ResponsesEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(self: Arc<Self>, request: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
        Box::pin(async move {
            let endpoint = tokio::select! {
                biased;
                () = ctx.cancellation().cancelled() => return Err(AgentError::Cancelled),
                endpoint = self.access.endpoint() => endpoint?,
            };
            let body = serde_json::to_vec(&body(&self.model, &request.history))
                .map_err(|e| AgentError::Provider(format!("could not encode the request: {e}")))?;
            let req = self
                .client
                .post(endpoint.url("responses"))
                .headers(endpoint.headers)
                .header("content-type", "application/json")
                .body(body);
            http::stream_sse(req, &ctx, |event| on_event(&event, &ctx)).await
        })
    }
}

/// The request: Krampsta's instructions (plus any system messages), the conversation as input
/// items, streamed and not stored on OpenAI's side.
fn body(model: &str, history: &[Message]) -> Value {
    let mut instructions = INSTRUCTIONS.to_owned();
    let mut input = Vec::new();
    for m in history {
        match m.role {
            MessageRole::System => {
                instructions.push_str("\n\n");
                instructions.push_str(&m.content);
            }
            MessageRole::User => {
                input.push(json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": m.content}]}));
            }
            MessageRole::Assistant if !m.content.is_empty() => input.push(
                json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": m.content}]}),
            ),
            MessageRole::Assistant | MessageRole::Tool => {}
        }
    }
    json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "stream": true,
        "store": false,
        // Think at Codex CLI's default effort, and stream a summary of it as reasoning events
        // (without an effort the backend may not reason at all).
        "reasoning": {"effort": "medium", "summary": "auto"},
        // Not stored, the backend leaves reasoning items (and their summary) out of the stream
        // unless asked for their encrypted content, as Codex CLI asks. Krampsta ignores it.
        "include": ["reasoning.encrypted_content"],
    })
}

/// Text and reasoning deltas go to the UI; `response.completed` ends the run; a failure fails it.
fn on_event(event: &SseEvent, ctx: &RunContext) -> Result<Flow, AgentError> {
    let v: Value = serde_json::from_str(event.data.trim())
        .map_err(|e| AgentError::Provider(format!("the provider sent an event Krampsta could not read ({e})")))?;
    let kind = v["type"].as_str().unwrap_or(event.event.as_str());
    trace(kind, &v);
    match kind {
        "response.output_text.delta" => {
            if let Some(text) = v["delta"].as_str().filter(|t| !t.is_empty()) {
                ctx.text(text);
            }
            Ok(Flow::Continue)
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if let Some(text) = v["delta"].as_str().filter(|t| !t.is_empty()) {
                ctx.reasoning(text);
            }
            Ok(Flow::Continue)
        }
        // Each later part of a summary starts a paragraph of its own.
        "response.reasoning_summary_part.added" => {
            if v["summary_index"].as_u64().is_some_and(|i| i > 0) {
                ctx.reasoning("\n\n");
            }
            Ok(Flow::Continue)
        }
        "response.completed" => Ok(Flow::Done),
        "response.failed" | "response.incomplete" => {
            let response = &v["response"];
            let why = response["error"]["message"]
                .as_str()
                .or_else(|| response["incomplete_details"]["reason"].as_str())
                .unwrap_or("the provider stopped the reply");
            Err(AgentError::Provider(why.to_owned()))
        }
        "error" => {
            let why = v["message"].as_str().or_else(|| v["error"]["message"].as_str()).unwrap_or("the provider failed");
            Err(AgentError::Provider(why.to_owned()))
        }
        _ => Ok(Flow::Continue),
    }
}

/// The event kinds this engine acts on; others pass by.
const HANDLED: [&str; 8] = [
    "response.output_text.delta",
    "response.reasoning_summary_text.delta",
    "response.reasoning_text.delta",
    "response.reasoning_summary_part.added",
    "response.completed",
    "response.failed",
    "response.incomplete",
    "error",
];

/// With `KRAMPSTA_TRACE_SSE=1`, writes each event's kind to stderr (and, for kinds this engine
/// ignores, its field names) to see what a backend sends. Never the values: they hold the
/// conversation.
fn trace(kind: &str, v: &Value) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("KRAMPSTA_TRACE_SSE").is_some_and(|x| x == "1")) {
        return;
    }
    if HANDLED.contains(&kind) {
        eprintln!("krampsta sse: {kind}");
    } else if kind == "response.created" {
        // The reasoning settings the backend applied (effort, summary): settings, not content.
        eprintln!("krampsta sse: {kind} (reasoning: {})", v["response"]["reasoning"]);
    } else if kind == "response.output_item.added" {
        // What kind of output starts ("reasoning", "message"…), without what it says.
        eprintln!("krampsta sse: {kind} (item type: {})", v["item"]["type"]);
    } else {
        let fields: Vec<&str> = v.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
        eprintln!("krampsta sse: {kind} (ignored; fields: {})", fields.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use krampsta_core::{MessageId, RunId};

    use super::*;

    #[test]
    fn the_conversation_becomes_input_items_and_system_text_joins_the_instructions() {
        let mut answered = Message::assistant(MessageId(3), RunId(1));
        answered.content = "hi".into();
        let mut empty = Message::assistant(MessageId(5), RunId(2));
        empty.content.clear();
        let history = vec![
            Message::new(MessageId(1), MessageRole::System, "Reply in Spanish."),
            Message::user(MessageId(2), "hello"),
            answered,
            Message::user(MessageId(4), "again"),
            empty,
        ];
        let b = body("gpt-6-sol", &history);
        assert_eq!(b["model"], "gpt-6-sol");
        assert_eq!(b["store"], false);
        assert_eq!(b["stream"], true);
        assert!(b["instructions"].as_str().unwrap().ends_with("Reply in Spanish."));
        let roles: Vec<&str> = b["input"].as_array().unwrap().iter().map(|i| i["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "user"]);
        assert_eq!(b["input"][1]["content"][0]["type"], "output_text");
    }
}
