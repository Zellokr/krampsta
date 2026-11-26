//! A long conversation is summed up before the run, through the host: a hidden request first,
//! then the run with the summary in place of what it covers.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use krampsta_agent::compact::{COMPACT_AT, SUMMARY_PROMPT};
use krampsta_agent::{AgentEngine, AgentError, AgentHost, BoxFuture, RunContext, RunRequest};
use krampsta_core::{AgentEvent, Message, MessageId, MessageRole, RunId, SessionId, Usage};

/// Answers a summary request with `summary` (or fails it), anything else with "reply"; keeps
/// every request's history.
struct Recorder {
    summary: Result<&'static str, &'static str>,
    seen: Mutex<Vec<Vec<Message>>>,
}

impl AgentEngine for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }

    fn run(self: Arc<Self>, request: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
        Box::pin(async move {
            let asks_summary = request.history.first().is_some_and(|m| m.content == SUMMARY_PROMPT);
            self.seen.lock().unwrap().push(request.history);
            ctx.usage(Usage { input: 10, output: 1 });
            match (asks_summary, self.summary) {
                (true, Ok(summary)) => ctx.text(summary),
                (true, Err(why)) => return Err(AgentError::Provider(why.into())),
                (false, _) => ctx.text("reply"),
            }
            Ok(())
        })
    }
}

fn long_history() -> Vec<Message> {
    let mut history = Vec::new();
    for n in 0..20 {
        history.push(Message::user(MessageId(2 * n + 1), format!("question {n} {}", "q".repeat(COMPACT_AT / 16))));
        let mut answer = Message::assistant(MessageId(2 * n + 2), RunId(n));
        answer.content = format!("answer {n}");
        history.push(answer);
    }
    history.push(Message::user(MessageId(41), "now"));
    history
}

fn run(engine: Arc<Recorder>) -> Vec<AgentEvent> {
    let mut host = AgentHost::new(engine);
    let run = host.next_run_id();
    host.start(RunRequest { session: SessionId(1), run, history: long_history(), effort: None }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while !events.last().is_some_and(AgentEvent::is_terminal) {
        assert!(Instant::now() < deadline, "timed out: {events:?}");
        events.extend(host.drain().into_iter().map(|e| e.event));
        thread::sleep(Duration::from_millis(2));
    }
    events
}

#[test]
fn the_run_gets_the_summary_instead_of_what_it_covers() {
    let engine = Arc::new(Recorder { summary: Ok("We sped up the parser."), seen: Mutex::default() });
    let events = run(engine.clone());

    assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })), "{events:?}");
    let compacted: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Compacted { summary, before, .. } => Some((summary.as_str(), *before)),
            _ => None,
        })
        .collect();
    assert_eq!(compacted, [("We sped up the parser.", MessageId(39))], "the previous turn stays whole");
    let text: String = events
        .iter()
        .filter_map(|e| if let AgentEvent::TextDelta { text, .. } = e { Some(text.as_str()) } else { None })
        .collect();
    assert_eq!(text, "reply", "the summary's text isn't part of the reply");
    let usage = events.iter().filter(|e| matches!(e, AgentEvent::Usage { .. })).count();
    assert_eq!(usage, 2, "the summary's tokens count too");

    let seen = engine.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0][1].content.ends_with("Krampsta: answer 18"), "the transcript ends before the kept turn");
    let run = &seen[1];
    let summary = run.iter().position(|m| m.role == MessageRole::Summary).unwrap();
    assert_eq!(run[summary].content, "We sped up the parser.");
    assert_eq!(run[summary + 1].id, MessageId(39));
}

#[test]
fn a_failed_summary_leaves_the_run_as_it_was() {
    let engine = Arc::new(Recorder { summary: Err("quota"), seen: Mutex::default() });
    let events = run(engine.clone());

    assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, AgentEvent::Compacted { .. })));
    let seen = engine.seen.lock().unwrap();
    assert_eq!(seen[1], long_history());
}
