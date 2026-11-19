//! Krampsta's domain model.
//!
//! Everything here is plain Rust data and pure state transitions: no UI framework, no async
//! runtime, no LLM provider. The UI (`krampsta-ui`) renders these types and the agent engines
//! (`krampsta-agent`) produce [`AgentEvent`]s that [`Session::apply`] folds into a session.

mod change;
mod event;
mod ids;
mod message;
mod session;

pub use change::{FileChange, splice_lines};
pub use event::AgentEvent;
pub use event::AgentStatus;
pub use event::RunEvent;
pub use event::ToolCall;
pub use event::ToolResult;
pub use event::Usage;
pub use ids::IdGen;
pub use ids::MessageId;
pub use ids::RunId;
pub use ids::SessionId;
pub use ids::ToolCallId;
pub use message::Message;
pub use message::MessageRole;
pub use session::Session;
pub use session::SessionError;
