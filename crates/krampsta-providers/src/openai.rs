//! The OpenAI Chat Completions engine, which also drives compatible servers (llama.cpp, vLLM,
//! Ollama, LM Studio…): same request, same `data: {…}` stream ending in `data: [DONE]`.

use std::sync::Arc;

use krampsta_agent::{AgentEngine, AgentError, BoxFuture, RunContext, RunRequest};
use krampsta_core::{Message, MessageRole};
use serde::{Deserialize, Serialize};

use crate::access::{Access, StaticAccess};
use crate::http::{self, Flow, HttpError};
use crate::sse::SseEvent;

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
            let endpoint = tokio::select! {
                biased;
                () = ctx.cancellation().cancelled() => return Err(AgentError::Cancelled),
                endpoint = self.access.endpoint() => endpoint?,
            };
            let body = ChatRequest { model: &self.model, stream: true, messages: wire_messages(&request.history) };
            let body = serde_json::to_vec(&body)
                .map_err(|e| AgentError::Provider(format!("could not encode the request: {e}")))?;
            let req = self
                .client
                .post(endpoint.url("chat/completions"))
                .headers(endpoint.headers)
                .header("content-type", "application/json")
                .body(body);
            http::stream_sse(req, &ctx, |event| on_event(&event, &ctx)).await
        })
    }
}

/// Handles one stream event: text goes to the UI, `[DONE]` ends the run, an error fails it.
fn on_event(event: &SseEvent, ctx: &RunContext) -> Result<Flow, AgentError> {
    let data = event.data.trim();
    if data == "[DONE]" {
        return Ok(Flow::Done);
    }
    let chunk: Chunk = serde_json::from_str(data)
        .map_err(|e| AgentError::Provider(format!("the provider sent an event Krampsta could not read ({e})")))?;
    if let Some(error) = chunk.error {
        return Err(AgentError::Provider(error.message.unwrap_or_else(|| "the provider reported an error".into())));
    }
    for choice in chunk.choices {
        let Delta { content, reasoning_content, reasoning } = choice.delta;
        if let Some(thought) = reasoning_content.or(reasoning).filter(|t| !t.is_empty()) {
            ctx.reasoning(thought);
        }
        if let Some(text) = content.filter(|t| !t.is_empty()) {
            ctx.text(text);
        }
    }
    Ok(Flow::Continue)
}

/// The conversation as the API expects it. Empty assistant turns (a run that failed before
/// writing) and tool output (no tool calling yet) are left out.
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
            Some(WireMessage { role, content: &m.content })
        })
        .collect()
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    stream: bool,
    messages: Vec<WireMessage<'a>>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
struct WireMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<Choice>,
    error: Option<StreamError>,
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
        let roles: Vec<_> = wire_messages(&history).iter().map(|m| (m.role, m.content)).collect();
        assert_eq!(roles, [("system", "be brief"), ("user", "first"), ("user", "again"), ("assistant", "hi")]);
    }
}
