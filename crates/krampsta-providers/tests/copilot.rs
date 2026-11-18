//! GitHub Copilot end to end against a fake GitHub + Copilot server: device sign-in, the saved
//! session, the token exchange, the model list, a streamed reply and signing out.

// The fake server's helpers are test code too: a panic there is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use krampsta_agent::{Account, AgentHost, DeviceCode, EngineCatalog, RunRequest};
use krampsta_core::{AgentEvent, Message, MessageId, SessionId};
use krampsta_providers::copilot::GitHub;
use krampsta_providers::secrets::{MemoryStore, SecretStore};
use krampsta_providers::{Catalog, Hosts};

/// Every request the fake server saw: "METHOD /path" and its lower-cased headers.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Serves GitHub's and Copilot's routes on one port until the test ends.
fn serve() -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let (api, log) = (base.clone(), seen.clone());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (line, headers) = read_head(&mut reader);
            let (body, kind) = route(&line, &api);
            log.lock().unwrap().push((line, headers));
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
        }
    });
    (base, seen)
}

fn read_head(reader: &mut BufReader<std::net::TcpStream>) -> (String, String) {
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    let mut headers = String::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse().unwrap();
        }
        headers.push_str(&lower);
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let mut parts = first.split_whitespace();
    (format!("{} {}", parts.next().unwrap_or(""), parts.next().unwrap_or("")), headers)
}

fn route(line: &str, api: &str) -> (String, &'static str) {
    let json = "application/json";
    match line {
        "POST /login/device/code" => (
            r#"{"device_code":"dc-1","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":1}"#.into(),
            json,
        ),
        "POST /login/oauth/access_token" => (r#"{"access_token":"gho_test","token_type":"bearer"}"#.into(), json),
        "GET /user" => (r#"{"login":"octocat"}"#.into(), json),
        "GET /copilot_internal/v2/token" => {
            (format!(r#"{{"token":"cop_tok","expires_at":4102444800,"endpoints":{{"api":"{api}/"}}}}"#), json)
        }
        "GET /models" => (
            r#"{"data":[{"id":"gpt-4.1","capabilities":{"type":"chat"},"model_picker_enabled":true},
                        {"id":"claude-sonnet-4.5","capabilities":{"type":"chat"},"policy":{"state":"enabled"}}]}"#
                .into(),
            json,
        ),
        "POST /chat/completions" => (
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hi from \"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"Copilot\"}}]}\n\ndata: [DONE]\n\n"
                .into(),
            "text/event-stream",
        ),
        _ => ("{}".into(), json),
    }
}

fn catalog(base: &str, store: Arc<MemoryStore>) -> Catalog {
    let hosts = Hosts { github: GitHub { web: base.into(), api: base.into() }, ..Hosts::default() };
    Catalog::build(|_| None, store, hosts).unwrap()
}

fn account(c: &Catalog) -> Option<Account> {
    c.providers().into_iter().find(|p| p.id == "copilot").and_then(|p| p.account().cloned())
}

fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(fut)
}

fn reply(host: &mut AgentHost) -> Vec<AgentEvent> {
    let run = host.next_run_id();
    host.start(RunRequest { session: SessionId(1), run, history: vec![Message::user(MessageId(1), "hello")] }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while !events.last().is_some_and(AgentEvent::is_terminal) {
        assert!(Instant::now() < deadline, "no reply: {events:?}");
        events.extend(host.drain().into_iter().map(|e| e.event));
        thread::sleep(Duration::from_millis(5));
    }
    events
}

#[test]
fn copilot_signs_in_remembers_the_session_lists_models_replies_and_signs_out() {
    let (base, seen) = serve();
    let store = Arc::new(MemoryStore::default());
    let c = catalog(&base, store.clone());
    assert_eq!(account(&c), Some(Account::SignedOut));
    assert!(c.engine("copilot", "gpt-4.1").is_err(), "no engine before signing in");

    // Sign in: the prompt gets GitHub's code, the future the account name.
    let codes: Arc<Mutex<Vec<DeviceCode>>> = Arc::default();
    let sink = codes.clone();
    let user = block_on(c.login("copilot", Arc::new(move |code| sink.lock().unwrap().push(code)))).unwrap();
    assert_eq!(user, "octocat");
    let expected = DeviceCode { url: "https://github.com/login/device".into(), code: "ABCD-1234".into() };
    assert_eq!(*codes.lock().unwrap(), [expected]);
    assert_eq!(account(&c), Some(Account::SignedIn { user: "octocat".into() }));
    let saved = store.get("copilot").unwrap().expect("the session is saved");
    assert!(saved.contains("gho_test") && saved.contains("octocat"), "{saved}");

    // A new start finds the session.
    let c = catalog(&base, store.clone());
    assert_eq!(account(&c), Some(Account::SignedIn { user: "octocat".into() }));

    assert_eq!(block_on(c.models("copilot")).unwrap(), ["gpt-4.1", "claude-sonnet-4.5"]);

    let mut host = AgentHost::new(c.engine("copilot", "gpt-4.1").unwrap());
    assert_eq!(host.engine_name(), "GitHub Copilot · gpt-4.1");
    let events = reply(&mut host);
    let text: String = events
        .iter()
        .filter_map(|e| if let AgentEvent::TextDelta { text, .. } = e { Some(text.as_str()) } else { None })
        .collect();
    assert_eq!(text, "Hi from Copilot");
    assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })));

    let seen = seen.lock().unwrap().clone();
    let (_, chat) = seen.iter().find(|(line, _)| line == "POST /chat/completions").expect("a chat request");
    assert!(chat.contains("authorization: bearer cop_tok"), "the Copilot token, not the GitHub one: {chat}");
    assert!(chat.contains("copilot-integration-id: vscode-chat"), "{chat}");
    let (_, exchange) = seen.iter().find(|(line, _)| line == "GET /copilot_internal/v2/token").unwrap();
    assert!(exchange.contains("authorization: token gho_test"), "{exchange}");

    block_on(c.logout("copilot")).unwrap();
    assert_eq!(account(&c), Some(Account::SignedOut));
    assert_eq!(store.get("copilot").unwrap(), None);
}

#[test]
fn a_cancelled_sign_in_reports_cancelled_and_saves_nothing() {
    let (base, _) = serve();
    let store = Arc::new(MemoryStore::default());
    let c = Arc::new(catalog(&base, store.clone()));
    let mut host = AgentHost::new(c.engine("mock", "simulated").unwrap());
    let login = c.clone();
    host.sign_in("copilot", move |prompt| login.login("copilot", prompt)).unwrap();

    // Wait for the code, then give up before the first poll.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while events.is_empty() {
        assert!(Instant::now() < deadline, "no code arrived");
        events = host.drain_accounts();
        thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(&events[0], krampsta_agent::AccountEvent::Code { code, .. } if code.code == "ABCD-1234"));
    assert!(host.cancel_sign_in());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = host.drain_accounts();
        if let Some(krampsta_agent::AccountEvent::SignedIn { result, .. }) = events.last() {
            assert_eq!(result, &Err(krampsta_agent::AgentError::Cancelled));
            break;
        }
        assert!(Instant::now() < deadline, "the sign-in never ended");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(store.get("copilot").unwrap(), None);
    assert_eq!(account(&c), Some(Account::SignedOut));
}
