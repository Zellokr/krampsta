//! An engine for OpenAI's Responses API (`POST {base}/responses`, streamed), which is what the
//! ChatGPT plan's backend speaks. Events arrive as `data: {"type": "response.…", …}`.

use std::sync::Arc;

use krampsta_agent::{AgentEngine, AgentError, BoxFuture, RunContext, RunRequest, ToolSpec};
use krampsta_core::{Message, MessageRole, ToolCall, ToolCallId, Usage};
use serde::Serialize;
use serde_json::{Value, json};

use crate::access::Access;
use crate::http::{self, Flow};
use crate::sse::SseEvent;
use crate::steps::{self, MAX_STEPS, Paragraphs};

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
            let mut shape = match self.shapes.clone() {
                Some(shapes) => tokio::select! {
                    biased;
                    () = ctx.cancellation().cancelled() => return Err(AgentError::Cancelled),
                    shape = shapes.shape(self.model.clone()) => shape,
                },
                None => ModelShape::default(),
            };
            // The user's choice wins over the model's default.
            if let Some(effort) = &request.effort {
                shape.effort = Some(effort.as_str().to_owned());
            }
            trace_shape(&self.model, &shape);
            // Built once per run; each step appends its output items and the tools' results.
            let reasoning = reasoning(&shape);
            let tools: Vec<Value> = ctx.tool_specs().iter().map(wire_tool).collect();
            let mut input = input(&request.history, steps::instructions(&ctx));
            let mut paragraphs = Paragraphs::default();
            for number in 0..MAX_STEPS {
                let endpoint = tokio::select! {
                    biased;
                    () = ctx.cancellation().cancelled() => return Err(AgentError::Cancelled),
                    endpoint = self.access.endpoint() => endpoint?,
                };
                trim_history(&mut input);
                let body = serde_json::to_vec(&Request::new(&self.model, &input, &reasoning, &tools))
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
                let mut step = Step::default();
                http::stream_sse(req, &ctx, |event| on_event(&event, &ctx, &mut step, &mut paragraphs)).await?;

                if step.calls.is_empty() {
                    return Ok(());
                }
                paragraphs.next_step();
                let calls: Vec<ToolCall> = step.calls.into_iter().map(|c| c.into_call(number)).collect();
                let results = steps::run_calls(&ctx, &calls).await?;
                input.extend(step.items);
                input.extend(
                    results
                        .into_iter()
                        .map(|r| json!({"type": "function_call_output", "call_id": r.call_id.0, "output": r.output})),
                );
            }
            Err(steps::too_many_steps())
        })
    }
}

/// What one response streamed that the next request needs: its output items, as the backend
/// sent them (reasoning with its encrypted content, the message, the calls), and the calls.
#[derive(Default)]
struct Step {
    items: Vec<Value>,
    calls: Vec<WireCall>,
}

/// A `function_call` output item.
struct WireCall {
    call_id: Option<String>,
    name: String,
    arguments: String,
}

impl WireCall {
    fn into_call(self, step: usize) -> ToolCall {
        ToolCall {
            id: self.call_id.map_or_else(|| steps::call_id(step, 0), ToolCallId),
            name: self.name,
            arguments: self.arguments,
        }
    }
}

/// The request, worded as Codex CLI words it: streamed and not stored on OpenAI's side, with
/// the reasoning settings the model's shape allows. Borrows everything: a step re-sends the
/// conversation without copying it.
#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    input: &'a [Value],
    stream: bool,
    store: bool,
    tool_choice: &'static str,
    /// A Lite model rejects parallel tool calls; Codex never asks for them.
    parallel_tool_calls: bool,
    reasoning: &'a serde_json::Map<String, Value>,
    /// Not stored, the backend leaves reasoning items (and their summary) out of the stream
    /// unless asked for their encrypted content, as Codex CLI asks. They go back as they came.
    include: [&'static str; 1],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    tools: &'a [Value],
}

impl<'a> Request<'a> {
    fn new(
        model: &'a str,
        input: &'a [Value],
        reasoning: &'a serde_json::Map<String, Value>,
        tools: &'a [Value],
    ) -> Self {
        Request {
            model,
            input,
            stream: true,
            store: false,
            tool_choice: "auto",
            parallel_tool_calls: false,
            reasoning,
            include: ["reasoning.encrypted_content"],
            tools,
        }
    }
}

/// A tool as the Responses API declares it.
fn wire_tool(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "name": spec.name,
        "description": spec.description,
        "parameters": spec.parameters,
        "strict": false,
    })
}

/// The reasoning settings `shape` allows.
fn reasoning(shape: &ModelShape) -> serde_json::Map<String, Value> {
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
    reasoning
}

/// The conversation as input items: the run's `instructions` (plus any system messages) as a
/// leading developer message, then each earlier turn's text (its tool traffic served that turn
/// and would only cost tokens now).
fn input(history: &[Message], mut instructions: String) -> Vec<Value> {
    let mut conversation = Vec::new();
    for m in history {
        match m.role {
            MessageRole::System => {
                instructions.push_str("\n\n");
                instructions.push_str(&m.content);
            }
            MessageRole::User => {
                let mut content = Vec::new();
                if !m.content.is_empty() || m.image_png.is_none() {
                    content.push(json!({"type": "input_text", "text": m.content}));
                }
                if let Some(png) = &m.image_png {
                    content.push(json!({"type": "input_image", "image_url": format!("data:image/png;base64,{png}")}));
                }
                conversation.push(json!({"type": "message", "role": "user", "content": content}));
            }
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
    input
}

/// Trim only older conversation turns, never the active turn's reasoning, calls or results.
fn trim_history(input: &mut Vec<Value>) {
    steps::trim_history(
        input,
        steps::HISTORY_BUDGET,
        |item| match item.get("role").and_then(Value::as_str) {
            Some("developer") => "system",
            Some("user") => "user",
            _ => "",
        },
        estimated_item_bytes,
    );
}

/// Estimate the JSON size without allocating a second copy of the request.
fn estimated_item_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len() + 2,
        Value::Array(items) => 2 + items.iter().map(|item| 1 + estimated_item_bytes(item)).sum::<usize>(),
        Value::Object(fields) => {
            2 + fields.iter().map(|(key, value)| key.len() + 4 + estimated_item_bytes(value)).sum::<usize>()
        }
        _ => 24,
    }
}

/// Text and reasoning deltas go to the UI; finished output items are kept for the next step;
/// `response.completed` ends the request; a failure fails it.
fn on_event(
    event: &SseEvent,
    ctx: &RunContext,
    step: &mut Step,
    paragraphs: &mut Paragraphs,
) -> Result<Flow, AgentError> {
    let v: Value = serde_json::from_str(event.data.trim())
        .map_err(|e| AgentError::Provider(format!("the provider sent an event Krampsta could not read ({e})")))?;
    let kind = v["type"].as_str().unwrap_or(event.event.as_str());
    trace(kind, &v);
    match kind {
        "response.output_text.delta" => {
            if let Some(text) = v["delta"].as_str().filter(|t| !t.is_empty()) {
                paragraphs.text(ctx, text);
            }
            Ok(Flow::Continue)
        }
        "response.output_item.done" => {
            // Cloned once per finished item (a few per response), not per delta.
            let Some(mut item) = v["item"].as_object().cloned() else { return Ok(Flow::Continue) };
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_owned);
                step.calls.push(WireCall {
                    call_id: text("call_id"),
                    name: text("name").unwrap_or_default(),
                    arguments: text("arguments").unwrap_or_default(),
                });
            }
            // Not stored, an item's id points at nothing on the backend: it goes back without one.
            item.remove("id");
            step.items.push(Value::Object(item));
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
        "response.completed" => {
            ctx.usage(usage(&v["response"]));
            Ok(Flow::Done)
        }
        "response.failed" | "response.incomplete" => {
            let response = &v["response"];
            ctx.usage(usage(response));
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

/// The tokens a finished response counted (`usage.input_tokens`, `usage.output_tokens`).
fn usage(response: &Value) -> Usage {
    let count = |field: &str| response["usage"][field].as_u64().unwrap_or(0);
    Usage { input: count("input_tokens"), output: count("output_tokens") }
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
const HANDLED: [&str; 9] = [
    "response.output_item.done",
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

    /// The first request of a run without tools, as JSON.
    fn body(model: &str, history: &[Message], shape: &ModelShape) -> Value {
        let (input, reasoning) = (input(history, INSTRUCTIONS.into()), reasoning(shape));
        serde_json::to_value(Request::new(model, &input, &reasoning, &[])).unwrap()
    }

    #[test]
    fn tools_go_as_functions_and_are_left_out_when_there_are_none() {
        let spec =
            ToolSpec { name: "search".into(), description: "Find".into(), parameters: json!({"type": "object"}) };
        let (input, reasoning) =
            (input(&[Message::user(MessageId(1), "hi")], INSTRUCTIONS.into()), reasoning(&ModelShape::default()));
        let tools = [wire_tool(&spec)];
        let with = serde_json::to_value(Request::new("m", &input, &reasoning, &tools)).unwrap();
        assert_eq!(
            with["tools"],
            json!([{"type": "function", "name": "search", "description": "Find", "parameters": {"type": "object"}, "strict": false}])
        );
        assert_eq!(with["parallel_tool_calls"], false);
        assert_eq!(body("m", &[], &ModelShape::default()).get("tools"), None);
    }

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
    fn history_budget_keeps_the_active_responses_tool_exchange() {
        let history = [
            Message::new(MessageId(1), MessageRole::System, "Keep this instruction."),
            Message::user(MessageId(2), "old"),
            Message::new(MessageId(3), MessageRole::Assistant, "x".repeat(steps::HISTORY_BUDGET)),
            Message::user(MessageId(4), "current"),
        ];
        let mut items = input(&history, INSTRUCTIONS.into());
        let active = [
            json!({"type": "reasoning", "encrypted_content": "opaque"}),
            json!({"type": "function_call", "call_id": "c", "name": "search", "arguments": "{}"}),
            json!({"type": "function_call_output", "call_id": "c", "output": "found"}),
        ];
        items.extend(active.clone());
        trim_history(&mut items);
        assert_eq!(items.len(), 5);
        assert!(items[0]["content"][0]["text"].as_str().unwrap().ends_with("Keep this instruction."));
        assert_eq!(items[1]["content"][0]["text"], "current");
        assert_eq!(&items[2..], &active);
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
