//! Krune's real agent engines.
//!
//! Each provider is an [`AgentEngine`](krune_agent::AgentEngine); the UI never knows which
//! one runs. [`Catalog`] lists the providers the environment allows (OpenAI, a custom
//! OpenAI-compatible server, Ollama, LM Studio, the simulated agent) and builds engines for
//! the provider and model the user picks. [`OpenAiEngine`] speaks the Chat Completions API.
//! The shared pieces are [`sse`] (an incremental `text/event-stream` parser) and [`http`]
//! (a cancellable streaming request with readable errors).
//!
//! Credentials come from the environment, never from the repository.

mod catalog;
pub mod http;
mod openai;
pub mod sse;

pub use catalog::{Catalog, DEFAULT_OPENAI_MODEL, LLAMA_CPP_URL, LM_STUDIO_URL, OLLAMA_URL};
pub use http::HttpError;
pub use openai::{DEFAULT_BASE_URL, OpenAiConfig, OpenAiEngine, list_models};

/// Why the configured provider could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("unknown KRUNE_PROVIDER `{0}` (expected openai, custom, llamacpp, ollama, lmstudio or mock)")]
    Unknown(String),
    #[error(transparent)]
    Http(#[from] HttpError),
}
