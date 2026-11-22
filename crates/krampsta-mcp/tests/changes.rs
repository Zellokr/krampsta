//! Servers added, stopped, started and removed while Krampsta runs, as Settings does it: the
//! change applies at once, `mcp.json` follows in the background, and the registry with it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use krampsta_agent::{NewServer, ServerKind, ServerState, ToolServers, Toolbox};
use krampsta_mcp::{McpServers, ToolSet, config};

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn state(servers: &McpServers, name: &str) -> Option<ServerState> {
    servers.statuses().into_iter().find(|s| s.name == name).map(|s| s.state)
}

fn saved(file: &Path) -> Vec<(String, bool)> {
    config::load(file).unwrap().into_iter().map(|s| (s.name, s.disabled)).collect()
}

fn fake(name: &str) -> NewServer {
    let command = format!("\"{}\"", env!("CARGO_BIN_EXE_krampsta-fake-mcp"));
    NewServer { name: name.into(), kind: ServerKind::Command, target: command, extra: "FAKE_TOKEN=t".into() }
}

#[test]
fn settings_changes_apply_at_once_and_reach_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mcp.json");
    let servers = McpServers::start(Vec::new(), Some(file.clone())).unwrap();
    let tools = ToolSet::new(None, servers.clone());
    let has_echo = || tools.specs().iter().any(|s| s.name == "mcp__fake__echo");

    servers.add(fake("fake")).unwrap();
    assert!(matches!(state(&servers, "fake"), Some(ServerState::Connecting | ServerState::Ready { .. })));
    wait_until("the new server connecting", || matches!(state(&servers, "fake"), Some(ServerState::Ready { .. })));
    assert!(has_echo(), "its tools join the registry");
    wait_until("the new server saved", || saved(&file) == [("fake".to_owned(), false)]);
    let raw = std::fs::read_to_string(&file).unwrap();
    assert!(raw.contains("FAKE_TOKEN"), "its environment is saved with it: {raw}");

    assert_eq!(servers.add(fake("fake")).unwrap_err(), "There is already a server named `fake`.");

    servers.set_enabled("fake", false).unwrap();
    assert_eq!(state(&servers, "fake"), Some(ServerState::Disabled));
    assert!(!has_echo(), "a stopped server's tools leave the registry");
    wait_until("the stop saved", || saved(&file) == [("fake".to_owned(), true)]);

    servers.set_enabled("fake", true).unwrap();
    wait_until("the server connecting again", || matches!(state(&servers, "fake"), Some(ServerState::Ready { .. })));
    assert!(has_echo());
    wait_until("the start saved", || saved(&file) == [("fake".to_owned(), false)]);

    servers.remove("fake").unwrap();
    assert_eq!(state(&servers, "fake"), None);
    assert!(!has_echo());
    wait_until("the removal saved", || saved(&file).is_empty());
    assert!(servers.take_problem().is_none());
}

#[test]
fn a_server_stopped_while_connecting_stays_stopped() {
    let servers = McpServers::start(Vec::new(), None).unwrap();
    servers.add(fake("quick")).unwrap();
    servers.set_enabled("quick", false).unwrap();
    // Long enough for the first attempt to finish, had it been kept.
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(state(&servers, "quick"), Some(ServerState::Disabled), "the stale attempt is dropped");
}

#[test]
fn a_change_that_cant_be_saved_says_why_and_still_applies() {
    let dir = tempfile::tempdir().unwrap();
    // A folder where the file should be: it can't be written.
    let file = dir.path().join("mcp.json");
    std::fs::create_dir(&file).unwrap();
    let servers = McpServers::start(Vec::new(), Some(file)).unwrap();
    let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = woken.clone();
    servers.set_notifier(Arc::new(move || _ = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let version = servers.version();
    let url = NewServer {
        name: "docs".into(),
        kind: ServerKind::Url,
        target: "http://127.0.0.1:9/mcp".into(),
        extra: String::new(),
    };
    servers.add(url).unwrap();
    assert!(servers.version() > version, "the UI sees the new server at once");
    let problem = std::sync::Mutex::new(None);
    wait_until("the failure reported", || {
        let taken = servers.take_problem();
        let found = taken.is_some();
        if found {
            *problem.lock().unwrap() = taken;
        }
        found
    });
    let problem = problem.into_inner().unwrap().unwrap();
    assert!(problem.contains("can't be"), "{problem}");
    assert!(state(&servers, "docs").is_some(), "the server is still there for this launch");
    assert!(woken.load(std::sync::atomic::Ordering::SeqCst) > 0);
}
