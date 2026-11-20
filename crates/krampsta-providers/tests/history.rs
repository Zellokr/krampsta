//! The history budget as requests really go out: a long conversation, a run that calls a tool
//! and then answers, both engines, against a fake server that keeps each request body.

// The fake server's helpers are test code too: a panic there is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use krampsta_agent::{AgentEngine, AgentHost, Call, RunRequest, ToolOutput, ToolSpec, Toolbox};
use krampsta_core::{AgentEvent, Message, MessageId, MessageRole, SessionId};
use krampsta_providers::access::StaticAccess;
use krampsta_providers::{OpenAiConfig, OpenAiEngine, ResponsesEngine};
use serde_json::{Value, json};

/// The engines' soft budget (`steps::HISTORY_BUDGET`), and what a request may carry beyond it:
/// the tools' schemas and JSON framing the estimate leaves out.
const BUDGET: usize = 128 * 1024;
const SLACK: usize = 8 * 1024;
/// Each earlier turn: a 8 KB question and a 8 KB answer, 64 of them (1 MB in all).
const TURNS: u64 = 64;
const TURN_TEXT: usize = 8 * 1024;

/// Answers each request with the next stream; hands out each request's body, raw.
fn serve(replies: Vec<String>) -> (String, Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (tx, bodies) = channel();
    thread::spawn(move || {
        for reply in replies {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            tx.send(body).ok();
            let mut out = stream;
            out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").unwrap();
            out.write_all(reply.as_bytes()).unwrap();
        }
    });
    (url, bodies)
}

/// One `look` tool that answers "looked".
struct Look;

impl Toolbox for Look {
    fn specs(&self) -> Arc<[ToolSpec]> {
        Arc::from([ToolSpec { name: "look".into(), description: "Look".into(), parameters: json!({"type": "object"}) }])
    }
    fn run(&self, _: &str, _: &str, _: &Call<'_>) -> Result<ToolOutput, String> {
        Ok(String::from("looked").into())
    }
}

/// 64 earlier turns (the first marked `OLDEST`, the last `NEWEST`), then the current question.
fn history() -> Vec<Message> {
    let mut history = Vec::new();
    for n in 0..TURNS {
        let mark = match n {
            0 => "OLDEST ",
            n if n == TURNS - 1 => "NEWEST ",
            _ => "",
        };
        history.push(Message::user(MessageId(2 * n), format!("{mark}{}", "q".repeat(TURN_TEXT))));
        history.push(Message::new(MessageId(2 * n + 1), MessageRole::Assistant, "a".repeat(TURN_TEXT)));
    }
    history.push(Message::user(MessageId(2 * TURNS), "CURRENT question"));
    history
}

/// Runs one turn with `engine`; the bodies of the requests it made.
fn run(engine: Arc<dyn AgentEngine>, bodies: &Receiver<Vec<u8>>, requests: usize) -> Vec<Vec<u8>> {
    let mut host = AgentHost::new(engine);
    host.set_toolbox(Some(Arc::new(Look)));
    let run = host.next_run_id();
    host.start(RunRequest { session: SessionId(1), run, history: history(), effort: None }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while !events.last().is_some_and(AgentEvent::is_terminal) {
        assert!(Instant::now() < deadline, "timed out: {events:?}");
        events.extend(host.drain().into_iter().map(|e| e.event));
        thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })), "{events:?}");
    (0..requests).map(|_| bodies.recv_timeout(Duration::from_secs(5)).unwrap()).collect()
}

/// What every request must carry, whatever was trimmed: a body within budget that kept the
/// instructions, the current question and the newest turns, and dropped the oldest.
fn within_budget(body: &[u8], what: &str) -> String {
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.len() <= BUDGET + SLACK, "{what}: {} bytes, over {} + {SLACK}", body.len(), BUDGET);
    assert!(body.len() >= BUDGET / 2, "{what}: {} bytes; the budget should be used, not emptied", body.len());
    assert!(text.contains("You are Krampsta"), "{what}: the instructions are kept");
    assert!(text.contains("CURRENT question"), "{what}: the current question is kept");
    assert!(text.contains("NEWEST "), "{what}: the newest earlier turn is kept");
    assert!(!text.contains("OLDEST "), "{what}: the oldest turn is dropped");
    text
}

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

#[test]
fn chat_completions_requests_stay_within_budget_and_keep_the_tool_exchange_whole() {
    let call = json!({"choices": [{"index": 0, "delta": {"tool_calls": [
        {"index": 0, "id": "call_1", "type": "function", "function": {"name": "look", "arguments": "{}"}}
    ]}}]});
    let text = json!({"choices": [{"index": 0, "delta": {"content": "Done."}}]});
    let (url, bodies) = serve(vec![sse(&[call]) + "data: [DONE]\n\n", sse(&[text]) + "data: [DONE]\n\n"]);
    let config = OpenAiConfig { base_url: url, api_key: None, model: "m".into() };
    let bodies = run(Arc::new(OpenAiEngine::new(config).unwrap()), &bodies, 2);

    within_budget(&bodies[0], "first request");
    within_budget(&bodies[1], "second request");
    let second: Value = serde_json::from_slice(&bodies[1]).unwrap();
    let messages = second["messages"].as_array().unwrap();
    let n = messages.len();
    assert_eq!(messages[n - 3]["content"], "CURRENT question");
    assert_eq!(messages[n - 2]["tool_calls"][0]["id"], "call_1", "the call right after the question");
    assert_eq!(
        (messages[n - 1]["role"].as_str(), messages[n - 1]["tool_call_id"].as_str()),
        (Some("tool"), Some("call_1"))
    );
    // Trimming drops whole turns: every kept answer follows its question.
    for pair in messages.windows(2) {
        if pair[1]["role"] == "assistant" && pair[1]["tool_calls"].is_null() {
            assert_eq!(pair[0]["role"], "user", "an answer kept without its question");
        }
    }
}

#[test]
fn responses_requests_stay_within_budget_and_keep_the_tool_exchange_whole() {
    let event = |kind: &str, mut rest: Value| {
        rest["type"] = json!(kind);
        rest
    };
    let call = json!({"type": "function_call", "call_id": "call_1", "name": "look", "arguments": "{}"});
    let first = sse(&[
        event("response.output_item.done", json!({"output_index": 0, "item": call})),
        event("response.completed", json!({"response": {}})),
    ]);
    let second = sse(&[
        event("response.output_text.delta", json!({"delta": "Done."})),
        event("response.completed", json!({"response": {}})),
    ]);
    let (url, bodies) = serve(vec![first, second]);
    let access = Arc::new(StaticAccess::new(&url, None).unwrap());
    let engine = ResponsesEngine::new(access, "m", reqwest::Client::new(), "ChatGPT");
    let bodies = run(Arc::new(engine), &bodies, 2);

    within_budget(&bodies[0], "first request");
    within_budget(&bodies[1], "second request");
    let second: Value = serde_json::from_slice(&bodies[1]).unwrap();
    let input = second["input"].as_array().unwrap();
    let n = input.len();
    assert_eq!(input[n - 3]["content"][0]["text"], "CURRENT question");
    assert_eq!(
        (input[n - 2]["type"].as_str(), input[n - 2]["call_id"].as_str()),
        (Some("function_call"), Some("call_1"))
    );
    assert_eq!(input[n - 1], json!({"type": "function_call_output", "call_id": "call_1", "output": "looked"}));
    assert_eq!(input[0]["role"], "developer", "the instructions lead");
}
