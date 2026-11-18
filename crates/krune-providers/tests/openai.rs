//! The OpenAI engine against a fake HTTP server that replays recorded streams.

// The fake server's helpers are test code too: a panic there is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use krune_agent::{AgentHost, RunRequest};
use krune_core::{AgentEvent, Message, MessageId, SessionId};
use krune_providers::{OpenAiConfig, OpenAiEngine};

/// A real reply from the Chat Completions API (trimmed ids), as the wire carries it.
const RECORDED: &str = concat!(
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
    ": keep-alive\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\", wörld\"},\"finish_reason\":null}]}\r\n\r\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"!\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);

/// What the fake server does with the one request it accepts.
struct Reply {
    status: &'static str,
    content_type: &'static str,
    /// Written one after another, flushed, with a pause between them.
    pieces: Vec<Vec<u8>>,
    /// After the last piece, keep the connection open until the client closes it.
    hang: bool,
}

impl Reply {
    /// An event stream cut into `piece_len`-byte pieces (which may split UTF-8 characters).
    fn sse(body: &str, piece_len: usize) -> Self {
        let pieces = body.as_bytes().chunks(piece_len).map(<[u8]>::to_vec).collect();
        Reply { status: "200 OK", content_type: "text/event-stream", pieces, hang: false }
    }
}

struct Server {
    url: String,
    /// The raw request (head + body) once received.
    request: Receiver<String>,
    /// Fires when the client closed the connection.
    closed: Receiver<()>,
}

fn serve(reply: Reply) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (req_tx, request) = channel();
    let (closed_tx, closed) = channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        req_tx.send(read_request(&mut reader)).ok();
        let mut out = stream;
        let head = format!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
            reply.status, reply.content_type
        );
        out.write_all(head.as_bytes()).unwrap();
        for piece in &reply.pieces {
            if out.write_all(piece).and_then(|()| out.flush()).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        if reply.hang {
            wait_for_close(&mut reader);
            closed_tx.send(()).ok();
        }
    });
    Server { url, request, closed }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> String {
    let mut head = String::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap();
        }
        head.push_str(&line);
        if line == "\r\n" || line.is_empty() {
            break;
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    head + &String::from_utf8(body).unwrap()
}

fn wait_for_close(reader: &mut BufReader<TcpStream>) {
    reader.get_ref().set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut buf = [0; 64];
    while matches!(reader.read(&mut buf), Ok(n) if n > 0) {}
}

fn host_for(server: &Server, key: Option<&str>) -> AgentHost {
    let config =
        OpenAiConfig { base_url: server.url.clone(), api_key: key.map(str::to_owned), model: "test-model".into() };
    AgentHost::new(Arc::new(OpenAiEngine::new(config).unwrap()))
}

fn start(host: &mut AgentHost, prompt: &str) -> krune_core::RunId {
    let run = host.next_run_id();
    host.start(RunRequest { session: SessionId(1), run, history: vec![Message::user(MessageId(1), prompt)] }).unwrap();
    run
}

/// Drains until `stop` matches the events so far (or fails after 10 s).
fn events_until(host: &mut AgentHost, stop: impl Fn(&[AgentEvent]) -> bool) -> Vec<AgentEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut all = Vec::new();
    loop {
        all.extend(host.drain().into_iter().map(|e| e.event));
        if stop(&all) {
            return all;
        }
        assert!(Instant::now() < deadline, "timed out; events so far: {all:?}");
        thread::sleep(Duration::from_millis(2));
    }
}

fn finished(events: &[AgentEvent]) -> bool {
    events.last().is_some_and(AgentEvent::is_terminal)
}

fn text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| if let AgentEvent::TextDelta { text, .. } = e { Some(text.as_str()) } else { None })
        .collect()
}

fn failure(events: &[AgentEvent]) -> String {
    match events.last() {
        Some(AgentEvent::Failed { error, .. }) => error.clone(),
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn a_recorded_stream_becomes_streamed_text_and_completes() {
    // Seven-byte pieces split events, lines and the JSON in arbitrary places.
    let server = serve(Reply::sse(RECORDED, 7));
    let mut host = host_for(&server, Some("sk-test"));
    let run = start(&mut host, "say hello");
    let events = events_until(&mut host, finished);

    assert_eq!(events.first(), Some(&AgentEvent::Started { run }));
    assert_eq!(text(&events), "Hello, wörld!");
    assert_eq!(events.last(), Some(&AgentEvent::Completed { run }));

    let request = server.request.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"), "{request}");
    assert!(request.to_ascii_lowercase().contains("authorization: bearer sk-test"));
    let body: serde_json::Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"], serde_json::json!([{ "role": "user", "content": "say hello" }]));
}

#[test]
fn local_servers_get_no_authorization_header() {
    let server = serve(Reply::sse("data: [DONE]\n\n", 64));
    let mut host = host_for(&server, None);
    start(&mut host, "hi");
    events_until(&mut host, finished);
    let request = server.request.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!request.to_ascii_lowercase().contains("authorization"));
}

#[test]
fn a_rejected_key_fails_with_the_providers_explanation() {
    let body = r#"{"error":{"message":"Incorrect API key provided: sk-bad.","type":"invalid_request_error"}}"#;
    let reply =
        Reply { status: "401 Unauthorized", content_type: "application/json", pieces: vec![body.into()], hang: false };
    let server = serve(reply);
    let mut host = host_for(&server, Some("sk-bad"));
    start(&mut host, "hi");
    let error = failure(&events_until(&mut host, finished));
    assert!(error.contains("rejected the credentials (HTTP 401)"), "{error}");
    assert!(error.contains("Incorrect API key provided"), "{error}");
}

#[test]
fn quota_errors_say_so() {
    let body = r#"{"error":{"message":"You exceeded your current quota."}}"#;
    let reply = Reply {
        status: "429 Too Many Requests",
        content_type: "application/json",
        pieces: vec![body.into()],
        hang: false,
    };
    let server = serve(reply);
    let mut host = host_for(&server, Some("sk-test"));
    start(&mut host, "hi");
    let error = failure(&events_until(&mut host, finished));
    assert!(error.contains("rate limit or quota"), "{error}");
}

#[test]
fn an_error_inside_the_stream_fails_the_run_after_the_text_so_far() {
    let stream = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
        "data: {\"error\":{\"message\":\"The server had an error while processing your request.\"}}\n\n",
    );
    let server = serve(Reply::sse(stream, 1024));
    let mut host = host_for(&server, None);
    start(&mut host, "hi");
    let events = events_until(&mut host, finished);
    assert_eq!(text(&events), "partial");
    assert!(failure(&events).contains("The server had an error"));
}

#[test]
fn an_unreachable_server_fails_with_a_connection_error() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let config =
        OpenAiConfig { base_url: format!("http://127.0.0.1:{port}/v1"), api_key: None, model: "test-model".into() };
    let mut host = AgentHost::new(Arc::new(OpenAiEngine::new(config).unwrap()));
    start(&mut host, "hi");
    let error = failure(&events_until(&mut host, finished));
    assert!(error.contains("could not reach"), "{error}");
}

#[test]
fn cancelling_mid_stream_closes_the_connection() {
    let first = "data: {\"choices\":[{\"delta\":{\"content\":\"thinking\"}}]}\n\n";
    let server =
        serve(Reply { status: "200 OK", content_type: "text/event-stream", pieces: vec![first.into()], hang: true });
    let mut host = host_for(&server, None);
    let run = start(&mut host, "write a novel");

    let started = Instant::now();
    let mut events = events_until(&mut host, |e| !text(e).is_empty());
    assert!(host.cancel(run));
    events.extend(events_until(&mut host, finished));
    assert_eq!(events.last(), Some(&AgentEvent::Cancelled { run }));
    assert!(started.elapsed() < Duration::from_secs(5));

    // The provider sees the request go away, so it stops generating (and billing).
    server.closed.recv_timeout(Duration::from_secs(5)).expect("the connection stayed open after cancelling");
}
