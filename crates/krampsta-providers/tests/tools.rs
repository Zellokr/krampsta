//! The agent loop: a reply asks for tools, Krampsta runs them and sends their results back,
//! until a reply asks for none. Against a fake server that answers each request in turn.
//!
//! The Responses API is tested with a real ChatGPT turn (`fixtures/chatgpt_tool_*.sse`). The
//! other streams follow the shapes the APIs document (Chat Completions' `delta.tool_calls`);
//! a real llama.cpp tool-call stream is still to be recorded.

// The fake server's helpers are test code too: a panic there is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use krampsta_agent::{AgentEngine, AgentHost, Call, RunRequest, ToolOutput, ToolSpec, Toolbox};
use krampsta_core::{AgentEvent, Message, MessageId, SessionId, Usage};
use krampsta_providers::access::StaticAccess;
use krampsta_providers::{OpenAiConfig, OpenAiEngine, ResponsesEngine};
use serde_json::{Value, json};

/// Answers `replies.len()` requests, one per connection, each with the next stream; hands out
/// each request's JSON body.
fn serve(replies: Vec<String>) -> (String, Receiver<Value>) {
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
            tx.send(serde_json::from_slice(&body).unwrap()).ok();
            let mut out = stream;
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            out.write_all(head.as_bytes()).unwrap();
            // Odd-sized pieces split events anywhere.
            for piece in reply.as_bytes().chunks(19) {
                out.write_all(piece).unwrap();
                out.flush().unwrap();
            }
        }
    });
    (url, bodies)
}

/// `echo` (and `run_command`, as recorded streams call it) answers with its arguments; `broken`
/// fails. Its project has rules ([`GUIDANCE`]).
struct Echo;

const GUIDANCE: &str = "<project_instructions>\nRun `just ci` before you finish.\n</project_instructions>";

impl Toolbox for Echo {
    fn specs(&self) -> Arc<[ToolSpec]> {
        let object = json!({"type": "object"});
        Arc::from([
            ToolSpec { name: "echo".into(), description: "Echo".into(), parameters: object.clone() },
            ToolSpec { name: "broken".into(), description: "Fails".into(), parameters: object },
        ])
    }

    fn guidance(&self) -> Option<Arc<str>> {
        Some(GUIDANCE.into())
    }

    fn run(&self, name: &str, arguments: &str, _: &Call<'_>) -> Result<ToolOutput, String> {
        match name {
            "echo" | "run_command" => Ok(format!("echo {arguments}").into()),
            _ => Err("it broke".into()),
        }
    }
}

fn run(engine: Arc<dyn AgentEngine>) -> Vec<AgentEvent> {
    let mut host = AgentHost::new(engine);
    host.set_toolbox(Some(Arc::new(Echo)));
    let run = host.next_run_id();
    let history = vec![Message::user(MessageId(1), "check")];
    host.start(RunRequest { session: SessionId(1), run, history, effort: None }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while !events.last().is_some_and(AgentEvent::is_terminal) {
        assert!(Instant::now() < deadline, "timed out: {events:?}");
        events.extend(host.drain().into_iter().map(|e| e.event));
        thread::sleep(Duration::from_millis(2));
    }
    events
}

fn text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| if let AgentEvent::TextDelta { text, .. } = e { Some(text.as_str()) } else { None })
        .collect()
}

/// Each finished call as (call id, output, failed).
fn results(events: &[AgentEvent]) -> Vec<(String, String, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallFinished { result, .. } => {
                Some((result.call_id.0.clone(), result.output.clone(), result.is_error))
            }
            _ => None,
        })
        .collect()
}

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

#[test]
fn chat_completions_runs_the_calls_and_sends_their_results_back() {
    let delta = |d: Value| json!({"choices": [{"index": 0, "delta": d}]});
    let first = sse(&[
        delta(json!({"content": "Let me check."})),
        // Two calls, their arguments in fragments, as OpenAI streams them.
        delta(
            json!({"tool_calls": [{"index": 0, "id": "call_a", "type": "function", "function": {"name": "echo", "arguments": ""}}]}),
        ),
        delta(json!({"tool_calls": [{"index": 0, "function": {"arguments": "{\"x\":"}}]})),
        delta(json!({"tool_calls": [{"index": 0, "function": {"arguments": "1}"}}]})),
        delta(
            json!({"tool_calls": [{"index": 1, "id": "call_b", "type": "function", "function": {"name": "broken", "arguments": "{}"}}]}),
        ),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
        json!({"choices": [], "usage": {"prompt_tokens": 50, "completion_tokens": 20}}),
    ]) + "data: [DONE]\n\n";
    let second = sse(&[
        delta(json!({"content": "All good."})),
        json!({"choices": [], "usage": {"prompt_tokens": 90, "completion_tokens": 5}}),
    ]) + "data: [DONE]\n\n";
    let (url, bodies) = serve(vec![first, second]);
    let config = OpenAiConfig { base_url: url, api_key: None, model: "m".into() };
    let events = run(Arc::new(OpenAiEngine::new(config).unwrap()));

    assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })), "{events:?}");
    assert_eq!(text(&events), "Let me check.\n\nAll good.", "the reply resumes in a new paragraph");
    assert_eq!(
        results(&events),
        [("call_a".into(), "echo {\"x\":1}".into(), false), ("call_b".into(), "it broke".into(), true)]
    );
    let usage: Vec<Usage> = events
        .iter()
        .filter_map(|e| if let AgentEvent::Usage { usage, .. } = e { Some(*usage) } else { None })
        .collect();
    assert_eq!(usage, [Usage { input: 50, output: 20 }, Usage { input: 90, output: 5 }], "one count per request");

    let first = bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        first["tools"][0],
        json!({"type": "function", "function": {"name": "echo", "description": "Echo", "parameters": {"type": "object"}}})
    );
    assert_eq!(first["messages"][0]["role"], "system", "with tools, the instructions lead");
    let instructions = first["messages"][0]["content"].as_str().unwrap();
    assert!(instructions.contains("read_file"));
    assert!(
        instructions.contains("Change files only with edit_file"),
        "the model is told not to edit through commands"
    );
    assert!(instructions.ends_with(GUIDANCE), "the project's rules follow Krampsta's: {instructions}");
    let mut second = bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    second["messages"].as_array_mut().unwrap().remove(0);
    assert_eq!(
        second["messages"],
        json!([
            {"role": "user", "content": "check"},
            {"role": "assistant", "content": "Let me check.", "tool_calls": [
                {"id": "call_a", "type": "function", "function": {"name": "echo", "arguments": "{\"x\":1}"}},
                {"id": "call_b", "type": "function", "function": {"name": "broken", "arguments": "{}"}},
            ]},
            {"role": "tool", "content": "echo {\"x\":1}", "tool_call_id": "call_a"},
            {"role": "tool", "content": "it broke", "tool_call_id": "call_b"},
        ])
    );
}

#[test]
fn the_responses_api_gets_its_output_items_back_without_ids_and_the_call_outputs() {
    let event = |kind: &str, rest: Value| {
        let mut e = rest;
        e["type"] = json!(kind);
        e
    };
    let reasoning = json!({"id": "rs_1", "type": "reasoning", "summary": [], "encrypted_content": "gAAA…"});
    let call = json!({"id": "fc_1", "type": "function_call", "status": "completed", "call_id": "call_1", "name": "echo", "arguments": "{\"q\":2}"});
    let first = sse(&[
        event("response.output_item.done", json!({"output_index": 0, "item": reasoning})),
        event("response.output_item.done", json!({"output_index": 1, "item": call})),
        event("response.completed", json!({"response": {"usage": {"input_tokens": 30, "output_tokens": 12}}})),
    ]);
    let second = sse(&[
        event("response.output_text.delta", json!({"delta": "Two."})),
        event("response.completed", json!({"response": {"usage": {"input_tokens": 60, "output_tokens": 3}}})),
    ]);
    let (url, bodies) = serve(vec![first, second]);
    let access = Arc::new(StaticAccess::new(&url, None).unwrap());
    let client = reqwest::Client::new();
    let events = run(Arc::new(ResponsesEngine::new(access, "m", client, "ChatGPT")));

    assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })), "{events:?}");
    assert_eq!(text(&events), "Two.");
    assert_eq!(results(&events), [("call_1".into(), "echo {\"q\":2}".into(), false)]);

    let first = bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(first["tools"][1]["name"], "broken");
    let second = bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    let input = second["input"].as_array().unwrap();
    assert_eq!(input.len(), 5, "developer, user, reasoning, call, output: {input:?}");
    let instructions = input[0]["content"][0]["text"].as_str().unwrap();
    assert!(instructions.contains("read_file"), "the tools hint");
    assert!(instructions.contains(GUIDANCE), "the project's rules: {instructions}");
    assert_eq!(input[2], json!({"type": "reasoning", "summary": [], "encrypted_content": "gAAA…"}));
    assert_eq!(
        input[3],
        json!({"type": "function_call", "status": "completed", "call_id": "call_1", "name": "echo", "arguments": "{\"q\":2}"})
    );
    assert_eq!(input[4], json!({"type": "function_call_output", "call_id": "call_1", "output": "echo {\"q\":2}"}));
}

/// A real turn from the ChatGPT backend (`gpt-6.2-sol`), recorded with `KRAMPSTA_RECORD_SSE` on
/// 2026-11-19 for "ejecuta `echo héllo ñandú`": the first stream calls `run_command`, its
/// arguments in 20 deltas; the second answers with the output. Only `safety_identifier` and
/// `prompt_cache_key` are redacted.
const CHATGPT_TOOL_CALL: &str = include_str!("fixtures/chatgpt_tool_call.sse");
const CHATGPT_TOOL_REPLY: &str = include_str!("fixtures/chatgpt_tool_reply.sse");

#[test]
fn a_recorded_chatgpt_turn_calls_the_tool_and_answers_with_its_output() {
    let (url, bodies) = serve(vec![CHATGPT_TOOL_CALL.into(), CHATGPT_TOOL_REPLY.into()]);
    let access = Arc::new(StaticAccess::new(&url, None).unwrap());
    let events = run(Arc::new(ResponsesEngine::new(access, "gpt-6.2-sol", reqwest::Client::new(), "ChatGPT")));

    assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })), "{events:?}");
    let call_id = "call_593fdadfb8b549da8dd1bcbed0770f0d";
    let arguments = r#"{"command":"echo héllo ñandú"}"#;
    assert_eq!(results(&events), [(call_id.into(), format!("echo {arguments}"), false)], "accents intact");
    assert_eq!(
        text(&events),
        "El comando terminó con código `0` e imprimió cada palabra en una línea:\n\n```text\nhéllo\nñandú\n```"
    );
    let usage: Vec<Usage> = events
        .iter()
        .filter_map(|e| if let AgentEvent::Usage { usage, .. } = e { Some(*usage) } else { None })
        .collect();
    assert_eq!(usage, [Usage { input: 1122, output: 23 }, Usage { input: 1170, output: 32 }]);

    let _first = bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    let second = bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    let input = second["input"].as_array().unwrap();
    let call = &input[input.len() - 2];
    assert_eq!(
        (call["type"].as_str(), call.get("id")),
        (Some("function_call"), None),
        "the call goes back without its id"
    );
    assert_eq!(call["arguments"], arguments);
    assert_eq!(
        input[input.len() - 1],
        json!({"type": "function_call_output", "call_id": call_id, "output": format!("echo {arguments}")})
    );
}
