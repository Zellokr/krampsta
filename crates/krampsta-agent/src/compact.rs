//! Summing up a long conversation before a run, so the model keeps its gist instead of the
//! engines dropping whole turns to fit [`HISTORY_BUDGET`].
//!
//! Before each run the host asks [`due`]: once what the engines would send passes
//! [`COMPACT_AT`], everything before the last [`KEEP_TURNS`] turns goes, as a transcript, to a
//! hidden request to the same engine without tools ([`summarize`]). Its reply becomes a
//! [`MessageRole::Summary`] ([`AgentEvent::Compacted`]), which the engines send instead of what
//! it covers ([`replay::start`]). The user still sees every message, and the chat marks where
//! the summary begins. A summary that fails leaves the run as it was: the engines trim.

use std::sync::Arc;

use krampsta_core::{AgentEvent, Message, MessageId, MessageRole};

use crate::engine::{AgentEngine, AgentError, RunContext, RunRequest};
use crate::replay::{self, HISTORY_BUDGET};

/// What the engines would send past which the conversation is summed up: room is left for the
/// run's own tool traffic.
pub const COMPACT_AT: usize = HISTORY_BUDGET * 3 / 4;
/// Turns before the current one that stay whole after a summary: the model keeps the exact
/// words of what it was just told.
pub const KEEP_TURNS: usize = 1;
/// The most of the transcript a summary request sends; its oldest part goes past it.
pub const TRANSCRIPT_MAX: usize = COMPACT_AT;

/// The instructions of the hidden request.
pub const SUMMARY_PROMPT: &str = "Sum up the conversation below between a user and Krampsta, a coding agent \
working in the user's project, so Krampsta can carry on without it. Keep: what the user wants and any \
decisions or preferences they stated; what was done and what was found, with the exact paths, names, commands \
and errors that matter; changes proposed and whether they were accepted; what is still open. Leave out \
greetings and anything the work no longer needs. Write it as plain notes, in the language of the conversation, \
in at most about 400 words. Don't answer or continue the conversation: only sum it up.";

/// The message the summary goes in before (the first one kept whole), when the conversation is
/// long enough to sum up and has something before the kept turns to sum up.
pub fn due(history: &[Message]) -> Option<MessageId> {
    if replay::estimated_bytes(history) < COMPACT_AT {
        return None;
    }
    let from = replay::start(history);
    // The current prompt and the kept turns before it.
    let before = history
        .iter()
        .enumerate()
        .skip(from)
        .filter(|(_, m)| m.role == MessageRole::User)
        .map(|(index, _)| index)
        .rev()
        .nth(KEEP_TURNS)?;
    let something = history[from..before].iter().any(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant));
    something.then(|| history[before].id)
}

/// The conversation up to `before`, from the last summary on, as text for the summary request:
/// tools' results as an older turn would send them, the newest part if it is too long.
pub fn transcript(history: &[Message], before: MessageId) -> String {
    let from = replay::start(history);
    let end = history.iter().position(|m| m.id == before).unwrap_or(history.len());
    let mut out = String::new();
    for m in &history[from.min(end)..end] {
        match m.role {
            MessageRole::Summary => push(&mut out, "Earlier summary", &m.content),
            MessageRole::User => push(&mut out, "User", &m.content),
            MessageRole::Assistant => {
                for r in replay::calls(m, replay::RECENT_TURNS + 1) {
                    let call = format!("{} {}\n→ {}", r.call.name, r.arguments, r.output);
                    push(&mut out, "Tool call", &call);
                }
                if !m.content.is_empty() {
                    push(&mut out, "Krampsta", &m.content);
                }
            }
            MessageRole::System | MessageRole::Tool => {}
        }
    }
    if out.len() > TRANSCRIPT_MAX {
        let mut cut = out.len() - TRANSCRIPT_MAX;
        while !out.is_char_boundary(cut) {
            cut += 1;
        }
        out.replace_range(..cut, "[the oldest part of the conversation is left out]\n");
    }
    out
}

fn push(out: &mut String, who: &str, text: &str) {
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(who);
    out.push_str(": ");
    out.push_str(text);
}

/// Asks `engine`, with no tools and no audience, to sum up `transcript`. Its token counts go to
/// `ctx`'s run; its text and reasoning go nowhere else.
pub async fn summarize(
    engine: Arc<dyn AgentEngine>,
    ctx: &RunContext,
    handle: &tokio::runtime::Handle,
    transcript: String,
) -> Result<String, AgentError> {
    let (events, received) = std::sync::mpsc::channel();
    let hidden = RunContext::new(
        ctx.session(),
        ctx.run(),
        events,
        Arc::new(|| {}),
        ctx.cancellation().child_token(),
        None,
        Default::default(),
    );
    let history =
        vec![Message::new(MessageId(0), MessageRole::System, SUMMARY_PROMPT), Message::user(MessageId(1), transcript)];
    let request = RunRequest { session: ctx.session(), run: ctx.run(), history, effort: None };
    // Its own task, as the run's: a panic in the engine is an error here, not a lost run.
    let result = match handle.spawn(engine.run(request, hidden)).await {
        Ok(result) => result,
        Err(join) => Err(AgentError::Provider(format!("engine task ended abnormally: {join}"))),
    };
    let mut summary = String::new();
    for event in received.try_iter() {
        match event.event {
            AgentEvent::TextDelta { text, .. } => summary.push_str(&text),
            AgentEvent::Usage { usage, .. } => ctx.usage(usage),
            _ => {}
        }
    }
    result?;
    ctx.check_cancelled()?;
    let summary = summary.trim();
    if summary.is_empty() {
        return Err(AgentError::Provider("the model wrote no summary".into()));
    }
    Ok(summary.to_owned())
}

/// Sums up `request`'s conversation if it is [`due`]: emits [`AgentEvent::Compacted`] and puts
/// the summary into the history the engine gets. A failed summary is reported and the run goes
/// on without it; only a cancellation stops it.
pub async fn before_run(
    engine: Arc<dyn AgentEngine>,
    mut request: RunRequest,
    ctx: &RunContext,
    handle: &tokio::runtime::Handle,
) -> Result<RunRequest, AgentError> {
    let Some(before) = due(&request.history) else { return Ok(request) };
    let text = transcript(&request.history, before);
    match summarize(engine, ctx, handle, text).await {
        Ok(summary) => {
            let at = request.history.iter().position(|m| m.id == before).unwrap_or(0);
            let message = Message::new(MessageId(u64::MAX), MessageRole::Summary, summary.clone());
            request.history.insert(at, message);
            ctx.emit(AgentEvent::Compacted { run: ctx.run(), summary, before });
            Ok(request)
        }
        Err(AgentError::Cancelled) => Err(AgentError::Cancelled),
        Err(_) if ctx.cancellation().is_cancelled() => Err(AgentError::Cancelled),
        Err(why) => {
            eprintln!("krampsta: the conversation could not be summed up ({why}); older turns are trimmed instead");
            Ok(request)
        }
    }
}

#[cfg(test)]
mod tests {
    use krampsta_core::{RunId, ToolCall, ToolCallId, ToolResult};

    use super::*;

    /// `turns` earlier question-and-answer pairs of `size` bytes each, then the current prompt.
    fn long(turns: u64, size: usize) -> Vec<Message> {
        let mut history = vec![Message::new(MessageId(0), MessageRole::System, "rules")];
        for n in 0..turns {
            history.push(Message::user(MessageId(2 * n + 1), format!("question {n} {}", "q".repeat(size))));
            let mut answer = Message::assistant(MessageId(2 * n + 2), RunId(n));
            answer.content = format!("answer {n}");
            history.push(answer);
        }
        history.push(Message::user(MessageId(2 * turns + 1), "now"));
        history
    }

    #[test]
    fn a_short_conversation_is_left_alone() {
        assert_eq!(due(&long(3, 1024)), None);
    }

    #[test]
    fn a_long_one_is_summed_up_before_the_last_turn() {
        let history = long(20, 8 * 1024);
        // The previous question (turn 19) stays whole.
        assert_eq!(due(&history), Some(MessageId(39)));
    }

    #[test]
    fn after_a_summary_only_what_follows_counts() {
        let mut history = long(20, 8 * 1024);
        // Just before the previous question: only the kept turn and the prompt follow it.
        history.insert(39, Message::new(MessageId(99), MessageRole::Summary, "done so far"));
        assert_eq!(due(&history), None);
        // However long the kept turn grows, there is nothing before it to sum up.
        history[40].content = "x".repeat(COMPACT_AT);
        assert_eq!(due(&history), None);
    }

    #[test]
    fn the_transcript_names_who_said_what_and_shortens_tools() {
        let mut history = long(2, 10);
        history[2].tool_calls.push(ToolCall {
            id: ToolCallId("c".into()),
            name: "read_file".into(),
            arguments: "{}".into(),
        });
        history[2].tool_results.push(ToolResult {
            call_id: ToolCallId("c".into()),
            output: "line\n".repeat(1000),
            is_error: false,
        });
        let text = transcript(&history, MessageId(3));
        assert!(text.starts_with("User: question 0"), "{text}");
        assert!(text.contains("Tool call: read_file {}\n→ line\n"));
        assert!(text.ends_with("Krampsta: answer 0"), "{text}");
        assert!(text.len() < 1024, "the tool's output is shortened: {}", text.len());
        assert!(!text.contains("rules"), "system messages aren't the conversation");
    }

    #[test]
    fn a_long_transcript_keeps_its_newest_part() {
        let history = long(40, 8 * 1024);
        let text = transcript(&history, MessageId(79));
        assert!(text.len() <= TRANSCRIPT_MAX + 64);
        assert!(text.starts_with("[the oldest part"));
        assert!(text.ends_with("answer 38"));
    }
}
