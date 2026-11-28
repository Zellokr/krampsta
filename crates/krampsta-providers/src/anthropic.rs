//! The Anthropic Messages API engine (Claude): `POST /v1/messages` with `stream: true`.
//!
//! One request per step: the reply streams text, adaptive thinking (shown folded, as a summary)
//! and `tool_use` blocks whose input arrives in pieces; the assistant's blocks go back as they
//! came (thinking with its signature) followed by one user message with every `tool_result`.
//! Earlier turns go as their answered calls and text ([`replay`]), a summary in the system
//! prompt. The prompt prefix is cached (`cache_control`), so each step pays little for it.

use std::sync::Arc;

use krampsta_agent::{
    AgentEngine, AgentError, BoxFuture, Effort, EffortLevel, RunContext, RunRequest, ToolSpec, replay,
};
use krampsta_core::{Message, MessageRole, ToolCall, ToolCallId, ToolResult, Usage};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::http::{self, Flow, HttpError};
use crate::sse::SseEvent;
use crate::steps::{self, MAX_STEPS, Paragraphs};

pub const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// The API version every request names.
const API_VERSION: &str = "2026-10-21";
/// Streamed, so a long reply can't time the request out.
const MAX_TOKENS: u32 = 64_000;
/// Lets the API rerun a declined request on another model inside the same call.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Models that take `fallbacks` (the current Fable, Opus and Sonnet).
const FALLBACK_MODELS: [&str; 5] =
    ["claude-fable-5-2", "claude-fable-5-1", "claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5"];

pub struct AnthropicEngine {
    base_url: String,
    headers: HeaderMap,
    model: String,
    client: reqwest::Client,
    name: String,
}

impl AnthropicEngine {
    /// An engine for `model` at `base_url` (up to and including `/v1`), shown as
    /// "<label> · <model>".
    pub fn new(
        base_url: &str,
        api_key: &str,
        model: &str,
        client: reqwest::Client,
        label: &str,
    ) -> Result<Self, AgentError> {
        Ok(AnthropicEngine {
            base_url: base_url.trim_end_matches('/').to_owned(),
            headers: headers(api_key)?,
            model: model.to_owned(),
            client,
            name: format!("{label} · {model}"),
        })
    }
}

/// `x-api-key` (marked sensitive, never printed) and the API version.
fn headers(api_key: &str) -> Result<HeaderMap, AgentError> {
    let mut key = HeaderValue::from_str(api_key)
        .map_err(|_| AgentError::Provider("the API key contains characters a header can't carry".into()))?;
    key.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", key);
    headers.insert("anthropic-version", HeaderValue::from_static(API_VERSION));
    Ok(headers)
}

/// The models the API lists (`GET /v1/models`), sorted.
pub async fn list_models(client: &reqwest::Client, base_url: &str, api_key: &str) -> Result<Vec<String>, HttpError> {
    let headers = headers(api_key).map_err(|e| HttpError::Client(e.to_string()))?;
    let url = format!("{}/models?limit=1000", base_url.trim_end_matches('/'));
    let json = http::get_json(client.get(url).headers(headers), MODELS_TIMEOUT).await?;
    let mut ids: Vec<String> =
        json["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str()).map(str::to_owned).collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

const MODELS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether `model` thinks adaptively and takes an effort: every current Claude model does; the
/// Claude 3 family and the 4.0–4.5 releases don't.
fn adaptive(model: &str) -> bool {
    const OLD: [&str; 6] = ["claude-3", "-4-0", "-4-1", "-4-5", "opus-4-2025", "sonnet-4-2025"];
    !OLD.iter().any(|old| model.contains(old))
}

/// The effort levels `model` takes, with what each is for; none for older models.
pub fn efforts(model: &str) -> Vec<EffortLevel> {
    if !adaptive(model) {
        return Vec::new();
    }
    [
        ("low", "Fast, for simple tasks"),
        ("medium", "Balanced speed and depth"),
        ("high", "Deeper reasoning for complex work"),
        ("xhigh", "Recommended for most coding and agentic work"),
        ("max", "The most thorough, when cost matters least"),
    ]
    .map(|(id, about)| EffortLevel { effort: Effort::new(id), description: Some(about.into()) })
    .into()
}

impl AgentEngine for AnthropicEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(self: Arc<Self>, request: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
        Box::pin(async move {
            let (system, mut messages) = conversation(&request.history, steps::instructions(&ctx));
            let tools: Vec<Value> = ctx.tool_specs().iter().map(wire_tool).collect();
            let mut paragraphs = Paragraphs::default();
            for number in 0..MAX_STEPS {
                let last = steps::is_last(number);
                if last {
                    wrap_up(&mut messages);
                }
                trim_history(&mut messages);
                let body = body(&self.model, &system, &messages, &tools, request.effort.as_ref(), last);
                let body = serde_json::to_vec(&body)
                    .map_err(|e| AgentError::Provider(format!("could not encode the request: {e}")))?;
                let mut req = self
                    .client
                    .post(format!("{}/messages", self.base_url))
                    .headers(self.headers.clone())
                    .header("content-type", "application/json");
                if FALLBACK_MODELS.contains(&self.model.as_str()) {
                    req = req.header("anthropic-beta", FALLBACK_BETA);
                }
                let mut step = Step::default();
                let result =
                    http::stream_sse(req.body(body), &ctx, |event| on_event(&event, &ctx, &mut step, &mut paragraphs))
                        .await;
                // Tokens a request read are spent even when it fails or is stopped after the count.
                ctx.usage(step.usage);
                result?;
                step.check_stop()?;

                let calls = step.calls(number);
                if calls.is_empty() {
                    return Ok(());
                }
                paragraphs.next_step();
                let results = steps::run_calls(&ctx, &calls).await?;
                messages.push(json!({"role": "assistant", "content": step.blocks}));
                messages.push(json!({"role": "user", "content": results.iter().map(tool_result).collect::<Vec<_>>()}));
            }
            Err(steps::too_many_steps())
        })
    }
}

/// The request for one step. `last`: the run's last, which may call no tools.
fn body(model: &str, system: &str, messages: &[Value], tools: &[Value], effort: Option<&Effort>, last: bool) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "system": system,
        "messages": messages,
        // The prefix (tools, system, earlier messages) is read from the cache on later steps.
        "cache_control": {"type": "ephemeral"},
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        if last {
            body["tool_choice"] = json!({"type": "none"});
        }
    }
    if adaptive(model) {
        body["thinking"] = json!({"type": "adaptive", "display": "summarized"});
        if let Some(effort) = effort {
            body["output_config"] = json!({"effort": effort.as_str()});
        }
    }
    if FALLBACK_MODELS.contains(&model) {
        // A request a safety classifier declines is rerun on the model the API picks for it.
        body["fallbacks"] = json!("default");
    }
    body
}

/// A tool as the Messages API declares it; its input streams as it is written.
fn wire_tool(spec: &ToolSpec) -> Value {
    json!({
        "name": spec.name,
        "description": spec.description,
        "input_schema": spec.parameters,
        "eager_input_streaming": true,
    })
}

/// A tool's result for the next request.
fn tool_result(result: &ToolResult) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": result.call_id.0,
        "content": result.output,
        "is_error": result.is_error,
    })
}

/// The system prompt (Krampsta's instructions, system messages and a summary) and the messages:
/// each earlier turn as its answered calls, each with its result, and then its text.
fn conversation(history: &[Message], mut system: String) -> (String, Vec<Value>) {
    let mut messages = Vec::with_capacity(history.len());
    let from = replay::start(history);
    for (index, (m, age)) in history.iter().zip(replay::ages(history)).enumerate() {
        if index < from && m.role != MessageRole::System {
            continue;
        }
        match m.role {
            MessageRole::System => {
                system.push_str("\n\n");
                system.push_str(&m.content);
            }
            MessageRole::Summary => {
                system.push_str("\n\n");
                system.push_str(replay::SUMMARY_HEADER);
                system.push_str("\n\n");
                system.push_str(&m.content);
            }
            MessageRole::User => {
                let mut content = Vec::new();
                if let Some(png) = &m.image_png {
                    content.push(json!({
                        "type": "image",
                        "source": {"type": "base64", "media_type": "image/png", "data": png.as_ref()},
                    }));
                }
                if !m.content.is_empty() || m.image_png.is_none() {
                    content.push(json!({"type": "text", "text": m.content}));
                }
                messages.push(json!({"role": "user", "content": content}));
            }
            MessageRole::Assistant => {
                for r in replay::calls(m, age) {
                    let input: Value = serde_json::from_str(&r.arguments).unwrap_or_else(|_| json!({}));
                    let id = &r.call.id.0;
                    messages.push(json!({
                        "role": "assistant",
                        "content": [{"type": "tool_use", "id": id, "name": r.call.name, "input": input}],
                    }));
                    messages.push(json!({
                        "role": "user",
                        "content": [{"type": "tool_result", "tool_use_id": id, "content": r.output}],
                    }));
                }
                if !m.content.is_empty() {
                    messages.push(json!({"role": "assistant", "content": [{"type": "text", "text": m.content}]}));
                }
            }
            MessageRole::Tool => {}
        }
    }
    (system, messages)
}

/// A user message that only carries tool results ends no turn: trimming never starts there.
fn role(message: &Value) -> &str {
    match message["role"].as_str() {
        Some("user") if message["content"][0]["type"] == "tool_result" => "tool",
        Some(role) => role,
        None => "",
    }
}

fn trim_history(messages: &mut Vec<Value>) {
    steps::trim_history(messages, steps::HISTORY_BUDGET, role, |m| m.to_string().len());
}

/// Tells the model, after the last result it gets, to answer without tools.
fn wrap_up(messages: &mut [Value]) {
    let last = messages.last_mut().filter(|m| role(m) == "tool");
    if let Some(Value::String(output)) =
        last.and_then(|m| m["content"].as_array_mut()).and_then(|c| c.last_mut()).map(|r| &mut r["content"])
    {
        steps::wrap_up(output);
    }
}

/// What one reply streamed: its content blocks as they go back, the tool calls among them,
/// how it stopped, and its tokens.
#[derive(Default)]
struct Step {
    blocks: Vec<Value>,
    /// The block being streamed, and the raw input of a `tool_use` one.
    open: Option<(Value, String)>,
    stop_reason: Option<String>,
    refusal: Option<String>,
    usage: Usage,
}

impl Step {
    fn calls(&self, step: usize) -> Vec<ToolCall> {
        self.blocks
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .enumerate()
            .map(|(index, b)| ToolCall {
                id: b["id"].as_str().map_or_else(|| steps::call_id(step, index), |id| ToolCallId(id.to_owned())),
                name: b["name"].as_str().unwrap_or_default().to_owned(),
                arguments: b["input"].to_string(),
            })
            .collect()
    }

    /// A reply that stopped before it was whole fails the run, rather than run half-written calls.
    fn check_stop(&self) -> Result<(), AgentError> {
        match self.stop_reason.as_deref() {
            Some("refusal") => {
                let why = self.refusal.as_deref().map_or_else(String::new, |c| format!(" ({c})"));
                Err(AgentError::Provider(format!("Claude declined this request{why}")))
            }
            Some("max_tokens") => Err(AgentError::Provider(format!(
                "the reply reached the {MAX_TOKENS}-token limit before it finished; ask for less at a time"
            ))),
            Some("model_context_window_exceeded") => Err(AgentError::Provider(
                "the conversation no longer fits the model's context; start a new session".into(),
            )),
            _ => Ok(()),
        }
    }

    fn close_block(&mut self) {
        let Some((mut block, input)) = self.open.take() else { return };
        if block["type"] == "tool_use" {
            // Input that doesn't parse (cut short) goes as an empty object: the tool says it's wrong.
            block["input"] = if input.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&input).unwrap_or_else(|_| json!({}))
            };
        }
        self.blocks.push(block);
    }
}

/// Text and thinking go to the UI as they stream; blocks are kept whole for the next step;
/// `message_stop` ends the request; an `error` event fails it.
fn on_event(
    event: &SseEvent,
    ctx: &RunContext,
    step: &mut Step,
    paragraphs: &mut Paragraphs,
) -> Result<Flow, AgentError> {
    let data = event.data.trim();
    if data.is_empty() {
        return Ok(Flow::Continue);
    }
    let v: Value = serde_json::from_str(data)
        .map_err(|e| AgentError::Provider(format!("the provider sent an event Krampsta could not read ({e})")))?;
    match v["type"].as_str().unwrap_or(event.event.as_str()) {
        "message_start" => {
            let usage = &v["message"]["usage"];
            let count = |field: &str| usage[field].as_u64().unwrap_or(0);
            step.usage.input =
                count("input_tokens") + count("cache_creation_input_tokens") + count("cache_read_input_tokens");
            step.usage.output = count("output_tokens");
        }
        "content_block_start" => {
            step.close_block();
            let mut block = v["content_block"].clone();
            if block["type"] == "text" {
                // Text streams in deltas; the start carries none worth keeping twice.
                block["text"] = json!("");
            }
            if block["type"] == "thinking" && step.blocks.iter().any(|b| b["type"] == "thinking") {
                ctx.reasoning("\n\n");
            }
            step.open = Some((block, String::new()));
        }
        "content_block_delta" => {
            let delta = &v["delta"];
            let Some((block, input)) = step.open.as_mut() else { return Ok(Flow::Continue) };
            match delta["type"].as_str() {
                Some("text_delta") => {
                    let text = delta["text"].as_str().unwrap_or_default();
                    if !text.is_empty() {
                        paragraphs.text(ctx, text);
                        append(&mut block["text"], text);
                    }
                }
                Some("thinking_delta") => {
                    let text = delta["thinking"].as_str().unwrap_or_default();
                    if !text.is_empty() {
                        ctx.reasoning(text);
                        append(&mut block["thinking"], text);
                    }
                }
                Some("signature_delta") => append(&mut block["signature"], delta["signature"].as_str().unwrap_or("")),
                Some("input_json_delta") => input.push_str(delta["partial_json"].as_str().unwrap_or_default()),
                _ => {}
            }
        }
        "content_block_stop" => step.close_block(),
        "message_delta" => {
            if let Some(reason) = v["delta"]["stop_reason"].as_str() {
                step.stop_reason = Some(reason.to_owned());
            }
            if let Some(category) = v["delta"]["stop_details"]["category"].as_str() {
                step.refusal = Some(category.to_owned());
            }
            if let Some(output) = v["usage"]["output_tokens"].as_u64() {
                step.usage.output = output;
            }
        }
        "message_stop" => {
            step.close_block();
            return Ok(Flow::Done);
        }
        "error" => {
            let why = v["error"]["message"].as_str().unwrap_or("the provider failed");
            return Err(AgentError::Provider(why.to_owned()));
        }
        _ => {}
    }
    Ok(Flow::Continue)
}

/// Appends to a string field, starting it if absent.
fn append(field: &mut Value, text: &str) {
    match field {
        Value::String(s) => s.push_str(text),
        other => *other = json!(text),
    }
}

#[cfg(test)]
mod tests {
    use krampsta_core::{MessageId, RunId};

    use super::*;

    #[test]
    fn current_models_think_and_take_an_effort_and_older_ones_dont() {
        let b = body("claude-opus-5-5", "be brief", &[], &[], Some(&Effort::new("xhigh")), false);
        assert_eq!(b["thinking"], json!({"type": "adaptive", "display": "summarized"}));
        assert_eq!(b["output_config"], json!({"effort": "xhigh"}));
        assert_eq!(b["fallbacks"], "default");
        assert_eq!(b["system"], "be brief");
        assert_eq!(b.get("tools"), None, "no tools, no tools field");
        assert_eq!(efforts("claude-opus-5-5").len(), 5);

        let old = body("claude-haiku-4-5", "x", &[], &[], Some(&Effort::new("high")), false);
        assert_eq!((old.get("thinking"), old.get("output_config"), old.get("fallbacks")), (None, None, None));
        assert!(efforts("claude-haiku-4-5").is_empty());
    }

    #[test]
    fn the_last_step_may_call_no_tools() {
        let tools = [json!({"name": "read_file"})];
        assert_eq!(body("claude-opus-5-5", "", &[], &tools, None, false).get("tool_choice"), None);
        assert_eq!(body("claude-opus-5-5", "", &[], &tools, None, true)["tool_choice"], json!({"type": "none"}));
    }

    #[test]
    fn earlier_turns_send_their_calls_and_text_and_a_summary_joins_the_system_prompt() {
        let mut answered = Message::assistant(MessageId(3), RunId(1));
        answered.content = "It says hi.".into();
        answered.tool_calls.push(ToolCall {
            id: ToolCallId("toolu_1".into()),
            name: "read_file".into(),
            arguments: r#"{"path":"a.txt"}"#.into(),
        });
        answered.tool_results.push(ToolResult {
            call_id: ToolCallId("toolu_1".into()),
            output: "hi".into(),
            is_error: false,
        });
        let history = [
            Message::user(MessageId(1), "forgotten"),
            Message::new(MessageId(2), MessageRole::Summary, "We looked at a.txt."),
            Message::user(MessageId(4), "what's in a.txt?"),
            answered,
            Message::user(MessageId(5), "and?"),
        ];
        let (system, messages) = conversation(&history, "rules".into());
        assert!(system.starts_with("rules\n\n") && system.ends_with("We looked at a.txt."));
        let shape: Vec<(&str, &str)> =
            messages.iter().map(|m| (role(m), m["content"][0]["type"].as_str().unwrap())).collect();
        assert_eq!(
            shape,
            [
                ("user", "text"),
                ("assistant", "tool_use"),
                ("tool", "tool_result"),
                ("assistant", "text"),
                ("user", "text")
            ]
        );
        assert_eq!(messages[1]["content"][0]["input"], json!({"path": "a.txt"}));
        assert!(!messages.iter().any(|m| m.to_string().contains("forgotten")), "the summary stands for it");
    }

    #[test]
    fn an_image_goes_before_its_text() {
        let image = Message { image_png: Some("iVBOR".into()), ..Message::user(MessageId(1), "what is this?") };
        let (_, messages) = conversation(&[image], String::new());
        assert_eq!(messages[0]["content"][0]["source"]["media_type"], "image/png");
        assert_eq!(messages[0]["content"][1]["text"], "what is this?");
    }

    #[test]
    fn a_refusal_and_a_cut_reply_fail_the_run() {
        let step = Step { stop_reason: Some("refusal".into()), refusal: Some("cyber".into()), ..Step::default() };
        assert_eq!(step.check_stop().unwrap_err().to_string(), "provider error: Claude declined this request (cyber)");
        let cut = Step { stop_reason: Some("max_tokens".into()), ..Step::default() };
        assert!(cut.check_stop().is_err());
        assert!(Step { stop_reason: Some("tool_use".into()), ..Step::default() }.check_stop().is_ok());
    }

    #[test]
    fn the_last_result_carries_the_note_to_wrap_up() {
        let mut messages = vec![json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "a", "content": "first"},
            {"type": "tool_result", "tool_use_id": "b", "content": "second"},
        ]})];
        wrap_up(&mut messages);
        assert_eq!(messages[0]["content"][0]["content"], "first");
        assert!(messages[0]["content"][1]["content"].as_str().unwrap().contains("can't call tools"));
    }

    #[test]
    fn the_key_is_never_printed() {
        let headers = headers("sk-ant-secret").unwrap();
        assert!(!format!("{headers:?}").contains("sk-ant-secret"));
    }
}
