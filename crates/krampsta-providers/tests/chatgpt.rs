//! A ChatGPT plan end to end against a fake OpenAI (auth + Codex backend): device sign-in, the
//! saved session, the model list, a streamed Responses reply, and a refresh after restarting
//! that saves the rotated refresh token.

// The fake server's helpers are test code too: a panic there is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use krampsta_agent::{Account, AgentHost, DeviceCode, EngineCatalog, RunRequest};
use krampsta_core::{AgentEvent, Message, MessageId, SessionId};
use krampsta_providers::chatgpt::OpenAiAccount;
use krampsta_providers::secrets::{MemoryStore, SecretStore};
use krampsta_providers::{Catalog, Hosts};
use serde_json::json;

/// A JWT whose payload is `claims` (unsigned: Krampsta doesn't check signatures).
fn jwt(claims: &serde_json::Value) -> String {
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!("e30.{}.sig", engine.encode(claims.to_string()))
}

/// Every request: "METHOD /path?query", lower-cased headers, body.
type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

fn serve() -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let log = seen.clone();
    let polls = Arc::new(AtomicUsize::new(0));
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (line, headers, body) = read_request(&mut reader);
            let (status, kind, reply) = route(&line, &body, &polls);
            log.lock().unwrap().push((line, headers, body));
            let out = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
            stream.write_all(out.as_bytes()).unwrap();
        }
    });
    (base, seen)
}

fn read_request(reader: &mut BufReader<std::net::TcpStream>) -> (String, String, String) {
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    let (mut headers, mut length) = (String::new(), 0);
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
    let line = format!("{} {}", parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    (line, headers, String::from_utf8(body).unwrap())
}

fn route(line: &str, body: &str, polls: &AtomicUsize) -> (&'static str, &'static str, String) {
    let ok = "200 OK";
    let json = "application/json";
    let path = line.split('?').next().unwrap_or(line);
    match path {
        "POST /api/accounts/deviceauth/usercode" => {
            (ok, json, r#"{"device_auth_id":"da-1","user_code":"QWER-5678","interval":"1"}"#.into())
        }
        // Pending once, then approved.
        "POST /api/accounts/deviceauth/token" if polls.fetch_add(1, Ordering::SeqCst) == 0 => {
            ("403 Forbidden", json, r#"{"error":"authorization_pending"}"#.into())
        }
        "POST /api/accounts/deviceauth/token" => {
            (ok, json, r#"{"authorization_code":"ac-1","code_challenge":"cc","code_verifier":"cv-1"}"#.into())
        }
        "POST /oauth/token" if body.contains("grant_type=authorization_code") => {
            let id = jwt(&json!({"email": "me@example.com", "https://api.openai.com/auth": {"chatgpt_account_id": "acc-1"}}));
            let access = jwt(&json!({"exp": 4_102_444_800_i64, "n": 1}));
            (ok, json, json!({"access_token": access, "refresh_token": "rt-1", "id_token": id}).to_string())
        }
        "POST /oauth/token" => {
            let access = jwt(&json!({"exp": 4_102_444_800_i64, "n": 2}));
            (ok, json, json!({"access_token": access, "refresh_token": "rt-2"}).to_string())
        }
        "GET /backend-api/codex/models" => (
            ok,
            json,
            r#"{"models":[{"slug":"gpt-6-sol","visibility":"list"},{"slug":"codex-auto-review","visibility":"hide"}]}"#
                .into(),
        ),
        "POST /backend-api/codex/responses" => (
            ok,
            "text/event-stream",
            concat!(
                "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n",
                "event: response.reasoning_summary_part.added\ndata: {\"type\":\"response.reasoning_summary_part.added\",\"item_id\":\"rs1\",\"output_index\":0,\"summary_index\":0,\"part\":{\"type\":\"summary_text\",\"text\":\"\"}}\n\n",
                "event: response.reasoning_summary_text.delta\ndata: {\"type\":\"response.reasoning_summary_text.delta\",\"item_id\":\"rs1\",\"output_index\":0,\"summary_index\":0,\"delta\":\"Greeting \"}\n\n",
                "event: response.reasoning_summary_text.delta\ndata: {\"type\":\"response.reasoning_summary_text.delta\",\"item_id\":\"rs1\",\"output_index\":0,\"summary_index\":0,\"delta\":\"the user.\"}\n\n",
                "event: response.reasoning_summary_part.added\ndata: {\"type\":\"response.reasoning_summary_part.added\",\"item_id\":\"rs1\",\"output_index\":0,\"summary_index\":1,\"part\":{\"type\":\"summary_text\",\"text\":\"\"}}\n\n",
                "event: response.reasoning_summary_text.delta\ndata: {\"type\":\"response.reasoning_summary_text.delta\",\"item_id\":\"rs1\",\"output_index\":0,\"summary_index\":1,\"delta\":\"Keep it short.\"}\n\n",
                "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello from \"}\n\n",
                "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"ChatGPT\"}\n\n",
                "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\"}}\n\n",
            )
            .into(),
        ),
        _ => ("404 Not Found", json, "{}".into()),
    }
}

fn catalog(base: &str, store: Arc<MemoryStore>) -> Catalog {
    let openai = OpenAiAccount { issuer: base.into(), api: format!("{base}/backend-api/codex") };
    Catalog::build(|_| None, store, Hosts { openai, ..Hosts::default() }).unwrap()
}

fn account(c: &Catalog) -> Option<Account> {
    c.providers().into_iter().find(|p| p.id == "chatgpt").and_then(|p| p.account().cloned())
}

fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(fut)
}

/// The reply's text, after checking its reasoning summary came through as reasoning.
fn reply_text(c: &Catalog) -> String {
    let mut host = AgentHost::new(c.engine("chatgpt", "gpt-6-sol").unwrap());
    let run = host.next_run_id();
    host.start(RunRequest { session: SessionId(1), run, history: vec![Message::user(MessageId(1), "hello")] }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while !events.last().is_some_and(AgentEvent::is_terminal) {
        assert!(Instant::now() < deadline, "no reply: {events:?}");
        events.extend(host.drain().into_iter().map(|e| e.event));
        thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })), "{events:?}");
    let thought: String = events
        .iter()
        .filter_map(|e| if let AgentEvent::ReasoningDelta { text, .. } = e { Some(text.as_str()) } else { None })
        .collect();
    assert_eq!(thought, "Greeting the user.\n\nKeep it short.", "one paragraph per summary part");
    events
        .iter()
        .filter_map(|e| if let AgentEvent::TextDelta { text, .. } = e { Some(text.as_str()) } else { None })
        .collect()
}

#[test]
fn a_chatgpt_plan_signs_in_lists_models_replies_and_refreshes_after_a_restart() {
    let (base, seen) = serve();
    let store = Arc::new(MemoryStore::default());
    let c = catalog(&base, store.clone());
    assert_eq!(account(&c), Some(Account::SignedOut));

    let codes: Arc<Mutex<Vec<DeviceCode>>> = Arc::default();
    let sink = codes.clone();
    let user = block_on(c.login("chatgpt", Arc::new(move |code| sink.lock().unwrap().push(code)))).unwrap();
    assert_eq!(user, "me@example.com");
    assert_eq!(*codes.lock().unwrap(), [DeviceCode { url: format!("{base}/codex/device"), code: "QWER-5678".into() }]);
    assert_eq!(account(&c), Some(Account::SignedIn { user: "me@example.com".into() }));
    let saved = store.get("chatgpt").unwrap().expect("the session is saved");
    assert!(saved.contains("rt-1") && saved.contains("acc-1"), "{saved}");
    assert!(!saved.contains("e30."), "access and id tokens stay out of the keychain: {saved}");

    {
        let seen = seen.lock().unwrap();
        let (_, _, exchange) =
            seen.iter().find(|(l, _, b)| l == "POST /oauth/token" && b.contains("authorization_code")).unwrap();
        assert!(exchange.contains("code=ac-1") && exchange.contains("code_verifier=cv-1"), "{exchange}");
        assert!(exchange.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"), "{exchange}");
    }

    assert_eq!(block_on(c.models("chatgpt")).unwrap(), ["gpt-6-sol"]);
    assert_eq!(reply_text(&c), "Hello from ChatGPT");
    {
        let seen = seen.lock().unwrap();
        let (_, headers, body) = seen.iter().find(|(l, _, _)| l == "POST /backend-api/codex/responses").unwrap();
        assert!(headers.contains("chatgpt-account-id: acc-1"), "{headers}");
        assert!(headers.contains("authorization: bearer e30."), "{headers}");
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!((body["model"].as_str(), body["store"].as_bool()), (Some("gpt-6-sol"), Some(false)));
        assert_eq!(body["reasoning"]["summary"], "auto", "asks for a summary of the thinking");
        assert!(
            !seen.iter().any(|(l, _, b)| l == "POST /oauth/token" && b.contains("refresh_token")),
            "no refresh yet"
        );
    }

    // After a restart only the refresh token is known: the first request refreshes, and the
    // rotated token replaces the saved one.
    let c = catalog(&base, store.clone());
    assert_eq!(account(&c), Some(Account::SignedIn { user: "me@example.com".into() }));
    assert_eq!(reply_text(&c), "Hello from ChatGPT");
    let refresh = seen
        .lock()
        .unwrap()
        .iter()
        .find(|(l, _, b)| l == "POST /oauth/token" && b.contains("grant_type=refresh_token"))
        .cloned();
    assert!(refresh.expect("a refresh").2.contains("refresh_token=rt-1"));
    assert!(store.get("chatgpt").unwrap().unwrap().contains("rt-2"), "the rotated refresh token is saved");

    block_on(c.logout("chatgpt")).unwrap();
    assert_eq!(account(&c), Some(Account::SignedOut));
    assert_eq!(store.get("chatgpt").unwrap(), None);
}
