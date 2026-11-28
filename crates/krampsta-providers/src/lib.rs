//! Krampsta's real agent engines.
//!
//! Each provider is an [`AgentEngine`](krampsta_agent::AgentEngine); the UI never knows which
//! one runs. [`Catalog`] lists the providers the environment allows (OpenAI, a custom
//! OpenAI-compatible server, Ollama, LM Studio, the simulated agent) and builds engines for
//! the provider and model the user picks. [`OpenAiEngine`] speaks the Chat Completions API.
//! The shared pieces are [`sse`] (an incremental `text/event-stream` parser) and [`http`]
//! (a cancellable streaming request with readable errors).
//!
//! Credentials come from the environment, never from the repository.

pub mod access;
mod accounts;
mod anthropic;
mod catalog;
pub mod chatgpt;
pub mod copilot;
pub mod http;
mod openai;
mod responses;
pub mod secrets;
pub mod sse;
mod steps;

pub use accounts::Hosts;
pub use anthropic::{ANTHROPIC_URL, AnthropicEngine};
pub use catalog::{Catalog, DEFAULT_OPENAI_MODEL, LLAMA_CPP_URL, LM_STUDIO_URL, OLLAMA_URL};
pub use http::HttpError;
pub use openai::{DEFAULT_BASE_URL, OpenAiConfig, OpenAiEngine, list_models};
pub use responses::ResponsesEngine;

/// Why the configured provider could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error(
        "unknown KRAMPSTA_PROVIDER `{0}` (expected openai, anthropic, chatgpt, copilot, custom, llamacpp, ollama, lmstudio or mock)"
    )]
    Unknown(String),
    #[error(transparent)]
    Http(#[from] HttpError),
}
