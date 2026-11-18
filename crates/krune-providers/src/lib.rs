//! Krune's real agent engines.
//!
//! Each provider is an [`AgentEngine`]; the UI never knows which one runs. Today:
//! [`OpenAiEngine`] for OpenAI and every server that speaks its Chat Completions API.
//! The shared pieces are [`sse`] (an incremental `text/event-stream` parser) and [`http`]
//! (a cancellable streaming request with readable errors).
//!
//! Credentials come from the environment, never from the repository.

pub mod http;
mod openai;
pub mod sse;

use std::sync::Arc;

use krune_agent::AgentEngine;

pub use http::HttpError;
pub use openai::{DEFAULT_BASE_URL, DEFAULT_MODEL, OpenAiConfig, OpenAiEngine};

/// Chooses the engine from the environment.
///
/// `KRUNE_PROVIDER=mock` forces the simulated agent; `KRUNE_PROVIDER=openai` (or simply having
/// `OPENAI_API_KEY` / `OPENAI_BASE_URL` set) picks [`OpenAiEngine`]. `Ok(None)` means no
/// provider is configured, so the caller keeps the mock.
pub fn engine_from_env() -> Result<Option<Arc<dyn AgentEngine>>, ProviderError> {
    let provider = std::env::var("KRUNE_PROVIDER").unwrap_or_default().trim().to_ascii_lowercase();
    match provider.as_str() {
        "mock" => Ok(None),
        "" | "openai" => match OpenAiConfig::from_env() {
            Some(config) => Ok(Some(Arc::new(OpenAiEngine::new(config)?))),
            None if provider.is_empty() => Ok(None),
            None => Err(ProviderError::Missing("set OPENAI_API_KEY, or OPENAI_BASE_URL for a local server")),
        },
        _ => Err(ProviderError::Unknown(provider)),
    }
}

/// Why the configured provider could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("unknown KRUNE_PROVIDER `{0}` (expected `openai` or `mock`)")]
    Unknown(String),
    #[error("the provider is not configured: {0}")]
    Missing(&'static str),
    #[error(transparent)]
    Http(#[from] HttpError),
}
