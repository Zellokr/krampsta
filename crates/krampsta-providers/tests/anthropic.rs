//! The Anthropic engine's loop against a fake server: a reply that thinks, says something and
//! calls a tool (its input in pieces), the call run and its result sent back with the blocks as
//! they came, then the answer.
//!
//! The streams follow the shapes the Messages API documents; a real Claude stream is still to
//! be recorded (`KRAMPSTA_RECORD_SSE`).

// The fake server's helpers are test code too: a panic there is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use krampsta_agent::{AgentEngine, AgentHost, Call, Effort, RunRequest, ToolOutput, ToolSpec, Toolbox};
use krampsta_core::{AgentEvent, Message, MessageId, SessionId, Usage};
use krampsta_providers::AnthropicEngine;
use serde_json::{Value, json};

/// One request as the server saw it: its header lines (lower-cased) and its JSON body.
struct Seen {
    headers: Vec<String>,
    body: Value,
}

/// Answers each request with the next stream, in odd-sized pieces.
fn serve(replies: Vec<String>) -> (String, Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (tx, seen) = channel();
    thread::spawn(move || {
        for reply in replies {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (mut headers, mut length) = (Vec::new(), 0);
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let lower = line.trim().to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                if lower.is_empty() {
                    break;
                }
                headers.push(lower);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            tx.send(Seen { headers, body: serde_json::from_slice(&body).unwrap() }).ok();
            let mut out = stream;
            out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").unwrap();
            for piece in reply.as_bytes().chunks(23) {
                out.write_all(piece).unwrap();
                out.flush().unwrap();
            }
        }
    });
    (url, seen)
}

/// `echo` answers with its arguments.
struct Echo;

impl Toolbox for Echo {
    fn specs(&self) -> Arc<[ToolSpec]> {
        let schema = json!({"type": "object", "properties": {"x": {"type": "number"}}});
        Arc::from([ToolSpec { name: "echo".into(), description: "Echo".into(), parameters: schema }])
    }

    fn run(&self, _: &str, arguments: &str, _: &Call<'_>) -> Result<ToolOutput, String> {
        Ok(format!("echo {arguments}").into())
    }
}

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())).collect()
}

fn delta(index: u64, delta: Value) -> Value {
    json!({"type": "content_block_delta", "index": index, "delta": delta})
}

fn start(index: u64, block: Value) -> Value {
    json!({"type": "content_block_start", "index": index, "content_block": block})
}

fn stop(index: u64) -> Value {
    json!({"type": "content_block_stop", "index": index})
}

fn message_start(input: u64, cached: u64) -> Value {
    json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": [],
        "usage": {"input_tokens": input, "cache_read_input_tokens": cached, "output_tokens": 1}}})
}

fn end(reason: &str, output: u64) -> [Value; 2] {
    [
        json!({"type": "message_delta", "delta": {"stop_reason": reason}, "usage": {"output_tokens": output}}),
        json!({"type": "message_stop"}),
    ]
}

fn run(engine: Arc<dyn AgentEngine>) -> Vec<AgentEvent> {
    let mut host = AgentHost::new(engine);
    host.set_toolbox(Some(Arc::new(Echo)));
    let run = host.next_run_id();
    let history = vec![Message::user(MessageId(1), "check")];
    host.start(RunRequest { session: SessionId(1), run, history, effort: Some(Effort::new("high")) }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while !events.last().is_some_and(AgentEvent::is_terminal) {
        assert!(Instant::now() < deadline, "timed out: {events:?}");
        events.extend(host.drain().into_iter().map(|e| e.event));
        thread::sleep(Duration::from_millis(2));
    }
    events
}

fn collect(events: &[AgentEvent], pick: impl Fn(&AgentEvent) -> Option<&str>) -> String {
    events.iter().filter_map(pick).collect()
}

#[test]
fn a_turn_thinks_calls_a_tool_sends_the_blocks_back_and_answers() {
    let mut first = vec![
        message_start(40, 900),
        start(0, json!({"type": "thinking", "thinking": "", "signature": ""})),
        delta(0, json!({"type": "thinking_delta", "thinking": "I should "})),
        delta(0, json!({"type": "thinking_delta", "thinking": "echo it."})),
        delta(0, json!({"type": "signature_delta", "signature": "sig=="})),
        stop(0),
        start(1, json!({"type": "text", "text": ""})),
        delta(1, json!({"type": "text_delta", "text": "Let me check."})),
        stop(1),
        start(2, json!({"type": "tool_use", "id": "toolu_a", "name": "echo", "input": {}})),
        delta(2, json!({"type": "input_json_delta", "partial_json": "{\"x\":"})),
        delta(2, json!({"type": "input_json_delta", "partial_json": " 1}"})),
        stop(2),
    ];
    first.extend(end("tool_use", 20));
    let mut second = vec![
        message_start(10, 1000),
        json!({"type": "ping"}),
        start(0, json!({"type": "text", "text": ""})),
        delta(0, json!({"type": "text_delta", "text": "All good."})),
        stop(0),
    ];
    second.extend(end("end_turn", 5));
    let (url, seen) = serve(vec![sse(&first), sse(&second)]);
    let engine = AnthropicEngine::new(&url, "sk-ant-test", "claude-opus-5-5", reqwest::Client::new(), "Anthropic");
    let events = run(Arc::new(engine.unwrap()));

    assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })), "{events:?}");
    let text = collect(&events, |e| if let AgentEvent::TextDelta { text, .. } = e { Some(text) } else { None });
    assert_eq!(text, "Let me check.\n\nAll good.");
    let thought = collect(&events, |e| if let AgentEvent::ReasoningDelta { text, .. } = e { Some(text) } else { None });
    assert_eq!(thought, "I should echo it.");
    let finished: Vec<_> = events
        .iter()
        .filter_map(|e| if let AgentEvent::ToolCallFinished { result, .. } = e { Some(result) } else { None })
        .collect();
    assert_eq!(finished.len(), 1);
    assert_eq!((finished[0].call_id.0.as_str(), finished[0].output.as_str()), ("toolu_a", r#"echo {"x":1}"#));
    let usage: Vec<Usage> = events
        .iter()
        .filter_map(|e| if let AgentEvent::Usage { usage, .. } = e { Some(*usage) } else { None })
        .collect();
    assert_eq!(
        usage,
        [Usage { input: 940, output: 20 }, Usage { input: 1010, output: 5 }],
        "cache reads count as input"
    );

    let first = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(first.headers.contains(&"x-api-key: sk-ant-test".to_owned()));
    assert!(first.headers.iter().any(|h| h.starts_with("anthropic-version: ")));
    assert!(first.headers.contains(&"anthropic-beta: server-side-fallback-2026-07-01".to_owned()));
    let body = &first.body;
    assert_eq!((body["model"].as_str(), body["stream"].as_bool()), (Some("claude-opus-5-5"), Some(true)));
    assert!(body["system"].as_str().unwrap().contains("read_file"), "with tools, Krampsta's instructions");
    assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    assert_eq!(body["output_config"], json!({"effort": "high"}));
    assert_eq!(body["messages"], json!([{"role": "user", "content": [{"type": "text", "text": "check"}]}]));

    let second = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    let messages = second.body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages[1],
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "I should echo it.", "signature": "sig=="},
            {"type": "text", "text": "Let me check."},
            {"type": "tool_use", "id": "toolu_a", "name": "echo", "input": {"x": 1}},
        ]}),
        "the blocks go back as they came"
    );
    assert_eq!(
        messages[2],
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_a", "content": "echo {\"x\":1}", "is_error": false}
        ]})
    );
}

#[test]
fn an_error_event_and_a_refusal_fail_the_run_readably() {
    let overloaded = sse(&[
        message_start(5, 0),
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
    ]);
    let mut refused = vec![message_start(5, 0)];
    refused.push(
        json!({"type": "message_delta", "delta": {"stop_reason": "refusal", "stop_details": {"category": "cyber"}}}),
    );
    refused.push(json!({"type": "message_stop"}));
    for (stream, why) in [(overloaded, "Overloaded"), (sse(&refused), "Claude declined this request (cyber)")] {
        let (url, _seen) = serve(vec![stream]);
        let engine = AnthropicEngine::new(&url, "k", "claude-sonnet-5-5", reqwest::Client::new(), "Anthropic").unwrap();
        let events = run(Arc::new(engine));
        match events.last() {
            Some(AgentEvent::Failed { error, .. }) => assert!(error.contains(why), "{error}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }
}
