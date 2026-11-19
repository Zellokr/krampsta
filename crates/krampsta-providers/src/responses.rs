//! An engine for OpenAI's Responses API (`POST {base}/responses`, streamed), which is what the
//! ChatGPT plan's backend speaks. Events arrive as `data: {"type": "response.…", …}`.

use std::sync::Arc;

use krampsta_agent::{AgentEngine, AgentError, BoxFuture, RunContext, RunRequest};
use krampsta_core::{Message, MessageRole};
use serde_json::{Value, json};

use crate::access::Access;
use crate::http::{self, Flow};
use crate::sse::SseEvent;

/// Krampsta's standing instructions to the model.
pub const INSTRUCTIONS: &str = "You are Krampsta, a coding assistant in a desktop app. Answer the user's \
questions about their code clearly and concisely, using Markdown and fenced code blocks.";

/// How a request to a model is worded, as its backend's catalog says (see [`Shapes`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelShape {
    /// The backend's "Responses Lite" mode: a header marks the request, and reasoning spans
    /// every turn.
    pub lite: bool,
    /// The reasoning effort to ask for (`low`, `medium`…); `None` leaves the model's own.
    pub effort: Option<String>,
    /// Whether the model takes `reasoning.summary` (and so can stream what it thought).
    pub summary: bool,
}

impl Default for ModelShape {
    /// For a model nobody described: a medium effort and a summary, as Codex CLI asks.
    fn default() -> Self {
        ModelShape { lite: false, effort: Some("medium".into()), summary: true }
    }
}

/// Describes models, looking them up when they aren't known yet.
pub trait Shapes: Send + Sync {
    fn shape(self: Arc<Self>, model: String) -> BoxFuture<'static, ModelShape>;
}

pub struct ResponsesEngine {
    access: Arc<dyn Access>,
    shapes: Option<Arc<dyn Shapes>>,
    model: String,
    client: reqwest::Client,
    name: String,
}

impl ResponsesEngine {
    pub fn new(access: Arc<dyn Access>, model: &str, client: reqwest::Client, label: &str) -> Self {
        ResponsesEngine { access, shapes: None, model: model.to_owned(), client, name: format!("{label} · {model}") }
    }

    /// Words each request as `shapes` describes the model (asked at the start of every run, so a
    /// catalog that arrives later still counts).
    pub fn with_shapes(mut self, shapes: Arc<dyn Shapes>) -> Self {
        self.shapes = Some(shapes);
        self
    }
}

impl AgentEngine for ResponsesEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(self: Arc<Self>, request: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
        Box::pin(async move {
            let shape = match self.shapes.clone() {
                Some(shapes) => tokio::select! {
                    biased;
                    () = ctx.cancellation().cancelled() => return Err(AgentError::Cancelled),
                    shape = shapes.shape(self.model.clone()) => shape,
                },
                None => ModelShape::default(),
            };
            trace_shape(&self.model, &shape);
            let endpoint = tokio::select! {
                biased;
                () = ctx.cancellation().cancelled() => return Err(AgentError::Cancelled),
                endpoint = self.access.endpoint() => endpoint?,
            };
            let body = serde_json::to_vec(&body(&self.model, &request.history, &shape))
                .map_err(|e| AgentError::Provider(format!("could not encode the request: {e}")))?;
            let mut req = self
                .client
                .post(endpoint.url("responses"))
                .headers(endpoint.headers)
                .header("content-type", "application/json");
            if shape.lite {
                req = req.header(crate::chatgpt::RESPONSES_LITE_HEADER, "true");
            }
            let req = req.body(body);
            http::stream_sse(req, &ctx, |event| on_event(&event, &ctx)).await
        })
    }
}

/// The request, worded as Codex CLI words it: Krampsta's instructions (plus any system messages)
/// as a leading developer message, then the conversation, streamed and not stored on OpenAI's
/// side, with the reasoning settings `shape` allows.
fn body(model: &str, history: &[Message], shape: &ModelShape) -> Value {
    let mut instructions = INSTRUCTIONS.to_owned();
    let mut conversation = Vec::new();
    for m in history {
        match m.role {
            MessageRole::System => {
                instructions.push_str("\n\n");
                instructions.push_str(&m.content);
            }
            MessageRole::User => conversation.push(
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": m.content}]}),
            ),
            MessageRole::Assistant if !m.content.is_empty() => conversation.push(
                json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": m.content}]}),
            ),
            MessageRole::Assistant | MessageRole::Tool => {}
        }
    }
    let mut input = vec![
        json!({"type": "message", "role": "developer", "content": [{"type": "input_text", "text": instructions}]}),
    ];
    input.extend(conversation);
    let mut reasoning = serde_json::Map::new();
    if let Some(effort) = &shape.effort {
        reasoning.insert("effort".into(), json!(effort));
    }
    if shape.summary {
        reasoning.insert("summary".into(), json!("auto"));
    }
    if shape.lite {
        reasoning.insert("context".into(), json!("all_turns"));
    }
    json!({
        "model": model,
        "input": input,
        "stream": true,
        "store": false,
        // As Codex sends them; a Lite model rejects parallel tool calls (and Krampsta has no tools yet).
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "reasoning": reasoning,
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

fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KRAMPSTA_TRACE_SSE").is_some_and(|x| x == "1"))
}

/// With tracing on, how a request to `model` is worded (from the backend's catalog).
fn trace_shape(model: &str, shape: &ModelShape) {
    if tracing() {
        eprintln!("krampsta sse: request to {model}: {shape:?}");
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
    if !tracing() {
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
        let b = body("gpt-6-sol", &history, &ModelShape::default());
        assert_eq!(b["model"], "gpt-6-sol");
        assert_eq!(b["store"], false);
        assert_eq!(b["stream"], true);
        assert_eq!(b.get("instructions"), None, "instructions lead the input, as Codex sends them");
        let roles: Vec<&str> = b["input"].as_array().unwrap().iter().map(|i| i["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["developer", "user", "assistant", "user"]);
        assert!(b["input"][0]["content"][0]["text"].as_str().unwrap().ends_with("Reply in Spanish."));
        assert_eq!(b["input"][2]["content"][0]["type"], "output_text");
        assert_eq!(b["reasoning"], json!({"effort": "medium", "summary": "auto"}));
    }

    #[test]
    fn the_model_shape_decides_the_reasoning_settings() {
        let history = [Message::user(MessageId(1), "hi")];
        let lite = ModelShape { lite: true, effort: Some("high".into()), summary: true };
        assert_eq!(
            body("m", &history, &lite)["reasoning"],
            json!({"effort": "high", "summary": "auto", "context": "all_turns"})
        );
        let plain = ModelShape { lite: false, effort: None, summary: false };
        assert_eq!(body("m", &history, &plain)["reasoning"], json!({}), "the model's own effort, no summary");
    }
}
