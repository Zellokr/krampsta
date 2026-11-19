//! A stand-in agent: thinks for a moment, "calls" a tool, and streams back a canned reply.
//!
//! It exercises the whole event path the real engines will use (start, tool calls, streamed
//! text, completion, failure, cancellation) without any network access.

use std::sync::Arc;
use std::time::Duration;

use krampsta_core::{ToolCall, ToolCallId, ToolResult};

use crate::engine::{AgentEngine, AgentError, BoxFuture, RunContext, RunRequest};

/// Prompts starting with this fail on purpose, to show the failed state.
pub const FAIL_COMMAND: &str = "/fail";

/// How long each phase of a mock run takes.
#[derive(Clone, Copy, Debug)]
pub struct MockConfig {
    /// Before anything is emitted ("thinking").
    pub think: Duration,
    /// Between the tool call starting and finishing.
    pub tool: Duration,
    /// Between streamed chunks of the reply.
    pub chunk: Duration,
}

impl Default for MockConfig {
    fn default() -> Self {
        MockConfig {
            think: Duration::from_millis(700),
            tool: Duration::from_millis(450),
            chunk: Duration::from_millis(28),
        }
    }
}

impl MockConfig {
    /// No waiting at all, for tests.
    pub fn instant() -> Self {
        MockConfig { think: Duration::ZERO, tool: Duration::ZERO, chunk: Duration::ZERO }
    }
}

#[derive(Debug, Default)]
pub struct MockAgent {
    config: MockConfig,
}

impl MockAgent {
    pub fn new(config: MockConfig) -> Self {
        MockAgent { config }
    }
}

impl AgentEngine for MockAgent {
    fn name(&self) -> &str {
        "mock"
    }

    fn run(self: Arc<Self>, request: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
        Box::pin(async move {
            let cfg = self.config;
            let prompt = request.prompt().to_owned();
            ctx.sleep(cfg.think).await?;

            if prompt.trim_start().starts_with(FAIL_COMMAND) {
                return Err(AgentError::Provider("simulated failure (you asked for it with /fail)".into()));
            }

            for chunk in chunks(REASONING) {
                ctx.check_cancelled()?;
                ctx.reasoning(chunk);
                if !cfg.chunk.is_zero() {
                    ctx.sleep(cfg.chunk).await?;
                }
            }

            let call = ToolCall {
                id: ToolCallId(format!("{}-call-1", ctx.run())),
                name: "list_files".into(),
                arguments: r#"{"path": "."}"#.into(),
            };
            ctx.tool_started(call.clone());
            ctx.sleep(cfg.tool).await?;
            ctx.tool_finished(ToolResult {
                call_id: call.id,
                output: "Cargo.toml\ncrates/\nREADME.md\n(simulated listing)".into(),
                is_error: false,
            });

            for chunk in chunks(&reply_for(&prompt, request.history.len())) {
                ctx.check_cancelled()?;
                ctx.text(chunk);
                if !cfg.chunk.is_zero() {
                    ctx.sleep(cfg.chunk).await?;
                }
            }
            Ok(())
        })
    }
}

/// What the simulated agent "thinks" before it acts, streamed as reasoning.
const REASONING: &str = "The user wants an answer about the workspace. I should look at the files first, \
                         then reply with a short example.";

/// The canned reply: echoes the request so it's clear the round trip worked.
fn reply_for(prompt: &str, turns: usize) -> String {
    let quoted: String = prompt.lines().next().unwrap_or("").chars().take(120).collect();
    format!(
        "This is a **simulated** reply from Krampsta's `MockAgent` (turn {turns}).\n\n\
         You asked: _{quoted}_\n\n\
         A real engine would now read the relevant files and propose changes. For example:\n\n\
         ```rust\nfn main() {{\n    println!(\"Hello from Krampsta\");\n}}\n```\n\n\
         Send a message starting with `/fail` to see how a failed run looks."
    )
}

/// Splits text into word-sized pieces (keeping whitespace), as a provider's stream would.
fn chunks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() && i > start {
            out.push(&text[start..i]);
            start = i;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentHost;
    use krampsta_core::{AgentEvent, Message, MessageId, SessionId};
    use std::time::Instant;

    fn run_to_end(host: &mut AgentHost, prompt: &str, cancel_after_start: bool) -> Vec<AgentEvent> {
        let run = host.next_run_id();
        let req =
            RunRequest { session: SessionId(1), run, history: vec![Message::user(MessageId(1), prompt)], effort: None };
        host.start(req).unwrap();
        if cancel_after_start {
            assert!(host.cancel(run));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut all = Vec::new();
        while !all.last().is_some_and(AgentEvent::is_terminal) {
            assert!(Instant::now() < deadline, "mock run never finished");
            all.extend(host.drain().into_iter().map(|e| e.event));
            std::thread::sleep(Duration::from_millis(2));
        }
        all
    }

    #[test]
    fn chunks_rejoin_to_the_original_text() {
        let text = "Hello  world\n\nnext line";
        assert_eq!(chunks(text).concat(), text);
        assert!(chunks(text).len() > 2);
    }

    #[test]
    fn mock_reply_thinks_calls_a_tool_streams_text_and_completes() {
        let mut host = AgentHost::new(Arc::new(MockAgent::new(MockConfig::instant())));
        let events = run_to_end(&mut host, "add a test", false);
        assert!(matches!(events.first(), Some(AgentEvent::Started { .. })));
        let thought: String = events
            .iter()
            .filter_map(|e| if let AgentEvent::ReasoningDelta { text, .. } = e { Some(text.as_str()) } else { None })
            .collect();
        assert_eq!(thought, REASONING);
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallStarted { .. })));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallFinished { .. })));
        let text: String = events
            .iter()
            .filter_map(|e| if let AgentEvent::TextDelta { text, .. } = e { Some(text.as_str()) } else { None })
            .collect();
        assert!(text.contains("add a test"), "the reply quotes the prompt: {text}");
        assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })));
    }

    #[test]
    fn fail_command_fails_the_run() {
        let mut host = AgentHost::new(Arc::new(MockAgent::new(MockConfig::instant())));
        let events = run_to_end(&mut host, "/fail please", false);
        assert!(matches!(events.last(), Some(AgentEvent::Failed { .. })));
    }

    #[test]
    fn cancelling_a_slow_run_ends_it_promptly() {
        let slow = MockConfig { think: Duration::from_secs(30), ..MockConfig::default() };
        let mut host = AgentHost::new(Arc::new(MockAgent::new(slow)));
        let started = Instant::now();
        let events = run_to_end(&mut host, "never mind", true);
        assert!(matches!(events.last(), Some(AgentEvent::Cancelled { .. })));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
