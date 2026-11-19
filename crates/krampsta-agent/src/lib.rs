//! Krampsta's agent layer.
//!
//! - [`AgentEngine`] is the contract every engine implements (today [`MockAgent`]; later
//!   OpenAI, Anthropic, Google, local models). An engine is an async function from a
//!   [`RunRequest`] to a stream of [`AgentEvent`](krampsta_core::AgentEvent)s, sent through its
//!   [`RunContext`], which also carries the run's cancellation token.
//! - [`AgentHost`] owns the async runtime, starts and cancels runs, guarantees each run's
//!   `Started` / terminal events, and hands events to the UI through a channel plus a
//!   [`Notifier`] (Rune's `Waker` fits it), so the UI thread never blocks.
//!
//! This crate knows nothing about the UI framework.

mod catalog;
mod engine;
mod host;
mod mock;

pub use catalog::{
    Account, DeviceCode, EngineCatalog, LoginPrompt, MOCK_MODEL, MOCK_PROVIDER, MockCatalog, Provider, ProviderKind,
    Selection,
};
pub use engine::AgentEngine;
pub use engine::AgentError;
pub use engine::BoxFuture;
pub use engine::RunContext;
pub use engine::RunRequest;
pub use engine::{Effort, EffortLevel};
pub use host::AgentHost;
pub use host::Notifier;
pub use host::{AccountEvent, ModelList};
pub use mock::MockAgent;
pub use mock::MockConfig;
pub use tokio_util::sync::CancellationToken;
