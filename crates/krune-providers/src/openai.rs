//! The OpenAI Chat Completions engine, which also drives compatible servers (llama.cpp, vLLM,
//! Ollama, LM Studio…): same request, same `data: {…}` stream ending in `data: [DONE]`.

use std::sync::Arc;

use krune_agent::{AgentEngine, AgentError, BoxFuture, RunContext, RunRequest};
use krune_core::{Message, MessageRole};
use serde::{Deserialize, Serialize};

use crate::http::{self, Flow, HttpError};
use crate::sse::SseEvent;

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";

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
    /// `OPENAI_API_KEY`, `OPENAI_BASE_URL` and `KRUNE_MODEL`. `None` when neither a key nor a
    /// base URL is set: there is nothing to talk to.
    pub fn from_env() -> Option<Self> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let var = |name: &str| var(name).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let api_key = var("OPENAI_API_KEY");
        let base_url = var("OPENAI_BASE_URL");
        if api_key.is_none() && base_url.is_none() {
            return None;
        }
        Some(OpenAiConfig {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_owned()),
            api_key,
            model: var("KRUNE_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
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
    config: OpenAiConfig,
    client: reqwest::Client,
    name: String,
}

impl OpenAiEngine {
    pub fn new(config: OpenAiConfig) -> Result<Self, HttpError> {
        let provider =
            if config.base_url.trim_end_matches('/') == DEFAULT_BASE_URL { "openai" } else { "openai-compatible" };
        let name = format!("{provider} · {}", config.model);
        Ok(OpenAiEngine { client: http::client()?, config, name })
    }
}

impl AgentEngine for OpenAiEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn run(self: Arc<Self>, request: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
        Box::pin(async move {
            let body =
                ChatRequest { model: &self.config.model, stream: true, messages: wire_messages(&request.history) };
            let body = serde_json::to_vec(&body)
                .map_err(|e| AgentError::Provider(format!("could not encode the request: {e}")))?;
            let mut req =
                self.client.post(self.config.endpoint()).header("content-type", "application/json").body(body);
            if let Some(key) = &self.config.api_key {
                req = req.bearer_auth(key);
            }
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
        .map_err(|e| AgentError::Provider(format!("the provider sent an event Krune could not read ({e})")))?;
    if let Some(error) = chunk.error {
        return Err(AgentError::Provider(error.message.unwrap_or_else(|| "the provider reported an error".into())));
    }
    for choice in chunk.choices {
        if let Some(text) = choice.delta.content.filter(|t| !t.is_empty()) {
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
}

#[derive(Deserialize)]
struct StreamError {
    message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use krune_core::{MessageId, RunId};

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        move |name| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    #[test]
    fn config_needs_a_key_or_a_base_url() {
        assert_eq!(OpenAiConfig::from_vars(vars(&[])), None);
        assert_eq!(OpenAiConfig::from_vars(vars(&[("OPENAI_API_KEY", "  ")])), None);
        let cloud = OpenAiConfig::from_vars(vars(&[("OPENAI_API_KEY", "sk-test")])).unwrap();
        assert_eq!((cloud.base_url.as_str(), cloud.model.as_str()), (DEFAULT_BASE_URL, DEFAULT_MODEL));
        let local = OpenAiConfig::from_vars(vars(&[
            ("OPENAI_BASE_URL", "http://localhost:11434/v1/"),
            ("KRUNE_MODEL", "qwen3"),
        ]))
        .unwrap();
        assert_eq!(local.api_key, None);
        assert_eq!(local.endpoint(), "http://localhost:11434/v1/chat/completions");
    }

    #[test]
    fn debug_output_hides_the_key() {
        let cfg = OpenAiConfig::from_vars(vars(&[("OPENAI_API_KEY", "sk-secret")])).unwrap();
        assert!(!format!("{cfg:?}").contains("sk-secret"));
    }

    #[test]
    fn engine_name_says_which_server_and_model() {
        let cfg = |url: &str| OpenAiConfig { base_url: url.into(), api_key: None, model: "m".into() };
        assert_eq!(OpenAiEngine::new(cfg(DEFAULT_BASE_URL)).unwrap().name(), "openai · m");
        assert_eq!(OpenAiEngine::new(cfg("http://localhost:8080/v1")).unwrap().name(), "openai-compatible · m");
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
