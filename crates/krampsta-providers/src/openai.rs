//! The OpenAI Chat Completions engine, which also drives compatible servers (llama.cpp, vLLM,
//! Ollama, LM Studio…): same request, same `data: {…}` stream ending in `data: [DONE]`.

use std::borrow::Cow;
use std::sync::Arc;

use krampsta_agent::{AgentEngine, AgentError, BoxFuture, RunContext, RunRequest, ToolSpec};
use krampsta_core::{Message, MessageRole, ToolCall, ToolCallId, ToolResult, Usage};
use serde::{Deserialize, Serialize};

use crate::access::{Access, StaticAccess};
use crate::http::{self, Flow, HttpError};
use crate::sse::SseEvent;
use crate::steps::{self, HISTORY_BUDGET, MAX_STEPS, Paragraphs};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Where and how to reach the server.
#[derive(Clone, PartialEq, Eq)]
pub struct OpenAiConfig {
    /// The API root, up to and including `/v1` (`http://localhost:11434/v1` for Ollama).
    pub base_url: String,
    /// Sent as a bearer token. Local servers usually need none.
    pub api_key: Option<String>,
    pub model: String,
}

impl OpenAiConfig {
    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url.trim_end_matches('/'))
    }
}

impl std::fmt::Debug for OpenAiConfig {
    // Never print the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<set>"))
            .field("model", &self.model)
            .finish()
    }
}

pub struct OpenAiEngine {
    access: Arc<dyn Access>,
    model: String,
    client: reqwest::Client,
    name: String,
}

impl OpenAiEngine {
    /// An engine with its own HTTP client, named after the server it talks to.
    pub fn new(config: OpenAiConfig) -> Result<Self, AgentError> {
        let label =
            if config.base_url.trim_end_matches('/') == DEFAULT_BASE_URL { "OpenAI" } else { "OpenAI-compatible" };
        Self::with_client(config, http::client()?, label)
    }

    /// An engine for a fixed server and key, sharing `client` (and its connection pool), shown
    /// as "<label> · <model>".
    pub fn with_client(config: OpenAiConfig, client: reqwest::Client, label: &str) -> Result<Self, AgentError> {
        let access = Arc::new(StaticAccess::new(&config.base_url, config.api_key.as_deref())?);
        Ok(Self::with_access(access, &config.model, client, label))
    }

    /// An engine whose server and credentials come from `access` on every request.
    pub fn with_access(access: Arc<dyn Access>, model: &str, client: reqwest::Client, label: &str) -> Self {
        OpenAiEngine { access, model: model.to_owned(), client, name: format!("{label} · {model}") }
    }
}

/// The models the server lists (`GET /models`), sorted. With `chat_only`, drops the ones that
/// can't chat (embeddings, speech, images…), which OpenAI lists alongside the rest.
pub async fn list_models(
    client: &reqwest::Client,
    config: &OpenAiConfig,
    chat_only: bool,
) -> Result<Vec<String>, HttpError> {
    let mut req = client.get(config.url("models"));
    if let Some(key) = &config.api_key {
        req = req.bearer_auth(key);
    }
    let json = http::get_json(req, MODELS_TIMEOUT).await?;
    let mut ids: Vec<String> = json["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str())
        .filter(|id| !chat_only || is_chat_model(id))
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// How long a model list may take (a local server that isn't running fails at once anyway).
const MODELS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

fn is_chat_model(id: &str) -> bool {
    const NOT_CHAT: [&str; 10] =
        ["embedding", "tts", "whisper", "dall-e", "moderation", "image", "audio", "realtime", "transcribe", "search"];
    !NOT_CHAT.iter().any(|n| id.contains(n))
}

impl AgentEngine for OpenAiEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(self: Arc<Self>, request: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
        Box::pin(async move {
            // Built once per run; each step only adds its own turn to the conversation.
            let tools: Vec<WireTool<'_>> = ctx.tool_specs().iter().map(WireTool::from).collect();
            let mut messages = wire_messages(&request.history);
            if !tools.is_empty() {
                // With tools the model must know it works in a project, and what the project's
                // rules are; without, it is a plain chat.
                let instructions = steps::instructions(&ctx);
                messages.insert(
                    0,
                    WireMessage { role: "system", content: Some(Cow::Owned(instructions)), ..WireMessage::default() },
                );
            }
            let mut paragraphs = Paragraphs::default();
            for number in 0..MAX_STEPS {
                trim_history(&mut messages, HISTORY_BUDGET);
                let endpoint = tokio::select! {
                    biased;
                    () = ctx.cancellation().cancelled() => return Err(AgentError::Cancelled),
                    endpoint = self.access.endpoint() => endpoint?,
                };
                let body = ChatRequest {
                    model: &self.model,
                    stream: true,
                    stream_options: StreamOptions { include_usage: true },
                    messages: &messages,
                    tools: &tools,
                };
                let body = serde_json::to_vec(&body)
                    .map_err(|e| AgentError::Provider(format!("could not encode the request: {e}")))?;
                let req = self
                    .client
                    .post(endpoint.url("chat/completions"))
                    .headers(endpoint.headers)
                    .header("content-type", "application/json")
                    .body(body);
                let mut step = Step::default();
                let result =
                    http::stream_sse(req, &ctx, |event| on_event(&event, &ctx, &mut step, &mut paragraphs)).await;
                // Tokens a request read are spent even when it fails or is stopped after the count.
                ctx.usage(step.counted.usage());
                result?;

                let calls = step.calls(number);
                if calls.is_empty() {
                    return Ok(());
                }
                paragraphs.next_step();
                let results = steps::run_calls(&ctx, &calls).await?;
                messages.push(WireMessage::assistant(step.text, &calls));
                messages.extend(results.into_iter().map(WireMessage::tool));
            }
            Err(steps::too_many_steps())
        })
    }
}

/// What one request's stream brought: its reply text, the tool calls it asked for (assembled
/// from their streamed pieces) and its token counts.
#[derive(Default)]
struct Step {
    text: String,
    calls: Vec<PartialCall>,
    counted: Counted,
}

/// A tool call as its pieces arrive: the id and name first, the arguments in fragments.
#[derive(Default)]
struct PartialCall {
    id: Option<String>,
    name: String,
    arguments: String,
}

impl Step {
    /// Adds a streamed piece of a call. Pieces name their call by `index`; a server that sends
    /// none (or reuses one for a call with a new id) starts a new call.
    fn add(&mut self, piece: DeltaCall) {
        let fresh =
            |id: &Option<String>, existing: &PartialCall| matches!((id, &existing.id), (Some(a), Some(b)) if a != b);
        let at = match piece.index {
            Some(i) if i < self.calls.len() && !fresh(&piece.id, &self.calls[i]) => i,
            Some(i) if i == self.calls.len() => i,
            _ => self.calls.len(),
        };
        if at == self.calls.len() {
            self.calls.push(PartialCall::default());
        }
        let call = &mut self.calls[at];
        if piece.id.is_some() {
            call.id = piece.id;
        }
        if let Some(f) = piece.function {
            if let Some(name) = f.name {
                call.name.push_str(&name);
            }
            if let Some(arguments) = f.arguments {
                call.arguments.push_str(&arguments);
            }
        }
    }

    /// The finished calls, in order (unnamed fragments dropped).
    fn calls(&mut self, step: usize) -> Vec<ToolCall> {
        std::mem::take(&mut self.calls)
            .into_iter()
            .filter(|c| !c.name.is_empty())
            .enumerate()
            .map(|(i, c)| ToolCall {
                id: c.id.map_or_else(|| steps::call_id(step, i), ToolCallId),
                name: c.name,
                arguments: c.arguments,
            })
            .collect()
    }
}

/// The token counts a stream carried. Servers count in one of two ways: a `usage` object in a
/// last chunk of its own (OpenAI, asked with `stream_options`; vLLM, Ollama, newer llama.cpp),
/// or llama.cpp's `timings` on the chunk that finishes. `usage` wins when both come.
#[derive(Default)]
struct Counted {
    usage: Option<Usage>,
    timings: Option<Usage>,
}

impl Counted {
    fn usage(&self) -> Usage {
        self.usage.or(self.timings).unwrap_or_default()
    }
}

/// Handles one stream event: text goes to the UI, counts are kept, `[DONE]` ends the run, an
/// error fails it.
fn on_event(
    event: &SseEvent,
    ctx: &RunContext,
    step: &mut Step,
    paragraphs: &mut Paragraphs,
) -> Result<Flow, AgentError> {
    let counted = &mut step.counted;
    let data = event.data.trim();
    if data == "[DONE]" {
        return Ok(Flow::Done);
    }
    let chunk: Chunk = serde_json::from_str(data)
        .map_err(|e| AgentError::Provider(format!("the provider sent an event Krampsta could not read ({e})")))?;
    if let Some(error) = chunk.error {
        return Err(AgentError::Provider(error.message.unwrap_or_else(|| "the provider reported an error".into())));
    }
    if let Some(u) = chunk.usage {
        counted.usage = Some(Usage { input: u.prompt_tokens, output: u.completion_tokens });
    }
    if let Some(t) = chunk.timings {
        // `prompt_n` counts only the prompt tokens it had to evaluate; `cache_n` the ones it reused.
        counted.timings = Some(Usage { input: t.prompt_n.saturating_add(t.cache_n), output: t.predicted_n });
    }
    for choice in chunk.choices {
        let Delta { content, reasoning_content, reasoning, tool_calls } = choice.delta;
        if let Some(thought) = reasoning_content.or(reasoning).filter(|t| !t.is_empty()) {
            ctx.reasoning(thought);
        }
        if let Some(text) = content.filter(|t| !t.is_empty()) {
            paragraphs.text(ctx, &text);
            step.text.push_str(&text);
        }
        for piece in tool_calls.into_iter().flatten() {
            step.add(piece);
        }
    }
    Ok(Flow::Continue)
}

/// The conversation as the API expects it. Earlier turns go as their text: empty assistant
/// turns (a run that failed before writing) are left out, and so is their tool traffic, which
/// served those turns and would only cost tokens now.
fn wire_messages(history: &[Message]) -> Vec<WireMessage<'_>> {
    history
        .iter()
        .filter_map(|m| {
            let role = match m.role {
                MessageRole::User => "user",
                MessageRole::Assistant if !m.content.is_empty() => "assistant",
                MessageRole::System => "system",
                MessageRole::Assistant | MessageRole::Tool => return None,
            };
            Some(WireMessage { role, content: Some(Cow::Borrowed(&m.content)), ..WireMessage::default() })
        })
        .collect()
}

fn trim_history(messages: &mut Vec<WireMessage<'_>>, budget: usize) {
    steps::trim_history(messages, budget, |message| message.role, WireMessage::estimated_bytes);
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    stream: bool,
    stream_options: StreamOptions,
    messages: &'a [WireMessage<'a>],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    tools: &'a [WireTool<'a>],
}

/// A tool as Chat Completions declares it: `{"type": "function", "function": {…}}`.
#[derive(Serialize)]
struct WireTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFunction<'a>,
}

#[derive(Serialize)]
struct WireFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a serde_json::Value,
}

impl<'a> From<&'a ToolSpec> for WireTool<'a> {
    fn from(spec: &'a ToolSpec) -> Self {
        WireTool {
            kind: "function",
            function: WireFunction { name: &spec.name, description: &spec.description, parameters: &spec.parameters },
        }
    }
}

/// A call as the assistant's turn carries it back to the model.
#[derive(Serialize, Debug, PartialEq, Eq)]
struct WireCall {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireCallFunction,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
struct WireCallFunction {
    name: String,
    arguments: String,
}

/// Asks for the token counts at the end of the stream (OpenAI sends none unless asked).
#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// One message of the conversation. History is borrowed; what the run adds is owned.
#[derive(Serialize, Debug, Default, PartialEq, Eq)]
struct WireMessage<'a> {
    role: &'static str,
    /// `null` for an assistant turn that only called tools.
    content: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

impl WireMessage<'_> {
    fn estimated_bytes(&self) -> usize {
        // Approximate JSON framing; text lengths are UTF-8 bytes before escaping.
        48 + self.content.as_deref().map_or(0, str::len)
            + self.tool_call_id.as_deref().map_or(0, str::len)
            + self
                .tool_calls
                .iter()
                .map(|call| 80 + call.id.len() + call.function.name.len() + call.function.arguments.len())
                .sum::<usize>()
    }

    /// The assistant's turn in a step that asked for tools.
    fn assistant(text: String, calls: &[ToolCall]) -> Self {
        let tool_calls = calls
            .iter()
            .map(|c| WireCall {
                id: c.id.0.clone(),
                kind: "function",
                function: WireCallFunction { name: c.name.clone(), arguments: c.arguments.clone() },
            })
            .collect();
        let content = (!text.is_empty()).then_some(Cow::Owned(text));
        WireMessage { role: "assistant", content, tool_calls, tool_call_id: None }
    }

    /// A tool's result, answering the call with its id.
    fn tool(result: ToolResult) -> Self {
        WireMessage {
            role: "tool",
            content: Some(Cow::Owned(result.output)),
            tool_calls: Vec::new(),
            tool_call_id: Some(result.call_id.0),
        }
    }
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<Choice>,
    error: Option<StreamError>,
    usage: Option<WireUsage>,
    timings: Option<Timings>,
}

#[derive(Deserialize)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

/// llama.cpp's per-reply statistics.
#[derive(Deserialize)]
struct Timings {
    #[serde(default)]
    prompt_n: u64,
    #[serde(default)]
    cache_n: u64,
    #[serde(default)]
    predicted_n: u64,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
}

#[derive(Deserialize, Default)]
struct Delta {
    content: Option<String>,
    /// Reasoning models' thinking, where compatible servers put it: `reasoning_content`
    /// (llama.cpp, vLLM, DeepSeek) or `reasoning` (Ollama, OpenRouter).
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    /// Pieces of the tool calls the reply asks for.
    tool_calls: Option<Vec<DeltaCall>>,
}

#[derive(Deserialize)]
struct DeltaCall {
    index: Option<usize>,
    id: Option<String>,
    function: Option<DeltaFunction>,
}

#[derive(Deserialize)]
struct DeltaFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct StreamError {
    message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use krampsta_core::{MessageId, RunId};

    fn cfg(url: &str, key: Option<&str>) -> OpenAiConfig {
        OpenAiConfig { base_url: url.into(), api_key: key.map(str::to_owned), model: "m".into() }
    }

    #[test]
    fn endpoints_tolerate_a_trailing_slash() {
        let local = cfg("http://localhost:11434/v1/", None);
        assert_eq!(local.url("chat/completions"), "http://localhost:11434/v1/chat/completions");
        assert_eq!(local.url("models"), "http://localhost:11434/v1/models");
    }

    #[test]
    fn debug_output_hides_the_key() {
        assert!(!format!("{:?}", cfg(DEFAULT_BASE_URL, Some("sk-secret"))).contains("sk-secret"));
    }

    #[test]
    fn engine_name_says_which_server_and_model() {
        assert_eq!(OpenAiEngine::new(cfg(DEFAULT_BASE_URL, None)).unwrap().name(), "OpenAI · m");
        assert_eq!(OpenAiEngine::new(cfg("http://localhost:8080/v1", None)).unwrap().name(), "OpenAI-compatible · m");
    }

    #[test]
    fn only_chat_models_survive_the_filter() {
        assert!(is_chat_model("gpt-4o-mini") && is_chat_model("o3") && is_chat_model("qwen3:4b"));
        assert!(!is_chat_model("text-embedding-3-small") && !is_chat_model("gpt-4o-mini-tts"));
    }

    #[test]
    fn history_trim_drops_whole_turns_and_preserves_system_messages() {
        let history = [
            Message::user(MessageId(1), "x".repeat(512)),
            Message::new(MessageId(2), MessageRole::System, "instructions"),
            Message::new(MessageId(3), MessageRole::Assistant, "old answer"),
            Message::user(MessageId(4), "previous question"),
            Message::new(MessageId(5), MessageRole::Assistant, "recent answer"),
            Message::user(MessageId(6), "current task"),
        ];
        let original = wire_messages(&history);
        let mut messages = wire_messages(&history);
        trim_history(&mut messages, HISTORY_BUDGET);
        assert_eq!(messages, original);

        // The old answer fits on its own, but its question doesn't.
        trim_history(&mut messages, 350);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0], original[1]);
        assert_eq!(messages[1..], original[3..]);
    }

    #[test]
    fn long_history_sends_fewer_bytes_without_splitting_tool_exchanges() {
        let mut history = vec![Message::new(MessageId(0), MessageRole::System, "instructions")];
        for id in 1..=64 {
            history.push(Message::user(MessageId(id), "x".repeat(8192)));
        }
        history.push(Message::user(MessageId(65), "current task"));
        let mut baseline = wire_messages(&history);
        let mut bounded = wire_messages(&history);
        let request_bytes = |messages: &[WireMessage<'_>]| {
            serde_json::to_vec(&ChatRequest {
                model: "m",
                stream: true,
                stream_options: StreamOptions { include_usage: true },
                messages,
                tools: &[],
            })
            .unwrap()
            .len()
        };

        for step in 0..=3 {
            if step > 0 {
                let call = ToolCall {
                    id: ToolCallId(format!("call-{step}")),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                };
                let exchange = || {
                    [
                        WireMessage::assistant(String::new(), std::slice::from_ref(&call)),
                        WireMessage {
                            role: "tool",
                            content: Some(Cow::Owned("y".repeat(48 * 1024))),
                            tool_call_id: Some(call.id.0.clone()),
                            ..WireMessage::default()
                        },
                    ]
                };
                baseline.extend(exchange());
                bounded.extend(exchange());
            }
            trim_history(&mut bounded, HISTORY_BUDGET);
            let active_len = 1 + step * 2;
            assert_eq!(bounded[0], baseline[0]);
            assert_eq!(bounded[bounded.len() - active_len..], baseline[baseline.len() - active_len..]);
            let before = request_bytes(&baseline);
            let after = request_bytes(&bounded);
            eprintln!("step {step}: {before} -> {after} JSON bytes");
            assert!(after < before / 2);
        }
        // The active turn alone is now larger than the budget; don't cut it.
        assert_eq!(bounded.len(), 8);
    }

    #[test]
    fn history_skips_empty_assistant_turns_and_tool_output() {
        let mut failed = Message::assistant(MessageId(2), RunId(1));
        failed.content.clear();
        let mut answered = Message::assistant(MessageId(4), RunId(2));
        answered.content = "hi".into();
        let history = vec![
            Message::new(MessageId(0), MessageRole::System, "be brief"),
            Message::user(MessageId(1), "first"),
            failed,
            Message::user(MessageId(3), "again"),
            answered,
            Message::new(MessageId(5), MessageRole::Tool, "output"),
        ];
        let wire = wire_messages(&history);
        let roles: Vec<_> = wire.iter().map(|m| (m.role, m.content.as_deref().unwrap_or_default())).collect();
        assert_eq!(roles, [("system", "be brief"), ("user", "first"), ("user", "again"), ("assistant", "hi")]);
    }
}
