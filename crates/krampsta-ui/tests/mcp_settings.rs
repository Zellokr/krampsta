#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The MCP servers in Settings, driven as a person would: add one through the form, stop it with
//! its switch, and remove it with two clicks, against servers that record what they are asked.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use krampsta_agent::{
    MockCatalog, MockConfig, NewServer, Notifier, ServerKind, ServerState, ServerStatus, ToolServers,
};
use krampsta_ui::{KrampstaApp, Screen, app, keys};
use rune::{Env, Ime, Runtime};

/// Servers that keep what they are told, as the real ones would, and remember it.
#[derive(Default)]
struct Recording {
    list: Mutex<Vec<ServerStatus>>,
    added: Mutex<Vec<NewServer>>,
    version: AtomicU64,
}

impl Recording {
    fn changed(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
    }
}

impl ToolServers for Recording {
    fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }
    fn statuses(&self) -> Vec<ServerStatus> {
        self.list.lock().unwrap().clone()
    }
    fn config_file(&self) -> Option<&Path> {
        Some(Path::new("mcp.json"))
    }
    fn set_notifier(&self, _: Notifier) {}

    fn add(&self, server: NewServer) -> Result<(), String> {
        if server.target.trim().is_empty() {
            return Err("Write the command that starts the server.".into());
        }
        let status =
            ServerStatus { name: server.name.clone(), reached: server.target.clone(), state: ServerState::Connecting };
        self.list.lock().unwrap().push(status);
        self.added.lock().unwrap().push(server);
        self.changed();
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        self.list.lock().unwrap().retain(|s| s.name != name);
        self.changed();
        Ok(())
    }

    fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        for s in self.list.lock().unwrap().iter_mut().filter(|s| s.name == name) {
            s.state = if enabled { ServerState::Connecting } else { ServerState::Disabled };
        }
        self.changed();
        Ok(())
    }
}

fn settle(rt: &mut Runtime<KrampstaApp>) {
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
}

fn has(rt: &Runtime<KrampstaApp>, key: &str) -> bool {
    rt.elements().into_iter().any(|e| e.key.as_deref() == Some(key) && !e.exiting)
}

fn click(rt: &mut Runtime<KrampstaApp>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn type_into(rt: &mut Runtime<KrampstaApp>, key: &str, text: &str) {
    click(rt, key);
    rt.ime(Ime::Commit(text.into()));
    settle(rt);
}

#[test]
fn a_server_is_added_stopped_and_removed_from_settings() {
    let servers = Arc::new(Recording::default());
    let mut state = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant()))).with_servers(servers.clone());
    state.screen = Screen::Settings;
    // Tall enough that the whole section shows without scrolling.
    let mut rt = Runtime::new(state, app::view, (1200.0, 1600.0), Env::new(true, 1.0));
    settle(&mut rt);
    assert!(has(&rt, keys::MCP_SERVERS_EMPTY), "none yet");

    click(&mut rt, keys::MCP_ADD_OPEN);
    assert!(has(&rt, keys::MCP_ADD_FORM));
    type_into(&mut rt, keys::MCP_ADD_NAME, "docs");
    type_into(&mut rt, keys::MCP_ADD_TARGET, "docs-server --stdio");
    type_into(&mut rt, keys::MCP_ADD_EXTRA, "DOCS_TOKEN=t");
    click(&mut rt, keys::MCP_ADD_SAVE);

    let added = servers.added.lock().unwrap().clone();
    assert_eq!(added.len(), 1);
    assert_eq!(
        (added[0].name.as_str(), added[0].kind, added[0].target.as_str()),
        ("docs", ServerKind::Command, "docs-server --stdio")
    );
    assert_eq!(added[0].extra, "DOCS_TOKEN=t");
    assert!(!has(&rt, keys::MCP_ADD_FORM), "the form closes");
    assert!(has(&rt, &keys::mcp_server("docs")), "and the server shows at once");

    click(&mut rt, &keys::mcp_server_toggle("docs"));
    assert_eq!(servers.statuses()[0].state, ServerState::Disabled);
    assert_eq!(rt.state.servers.list[0].state, ServerState::Disabled, "the card follows");

    click(&mut rt, &keys::mcp_server_remove("docs"));
    assert!(has(&rt, &keys::mcp_server_remove_confirm("docs")), "one click only asks");
    assert_eq!(servers.statuses().len(), 1);
    click(&mut rt, &keys::mcp_server_remove_confirm("docs"));
    assert!(servers.statuses().is_empty());
    assert!(!has(&rt, &keys::mcp_server("docs")));
}

#[test]
fn a_server_the_servers_refuse_keeps_the_form_open_and_says_why() {
    let servers = Arc::new(Recording::default());
    let mut state = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant()))).with_servers(servers.clone());
    state.screen = Screen::Settings;
    let mut rt = Runtime::new(state, app::view, (1200.0, 1600.0), Env::new(true, 1.0));
    settle(&mut rt);
    click(&mut rt, keys::MCP_ADD_OPEN);
    type_into(&mut rt, keys::MCP_ADD_NAME, "docs");
    type_into(&mut rt, keys::MCP_ADD_TARGET, "   ");
    // The button stays off while the command is blank; Enter in the field still tries.
    rt.update(|s, _| s.add_server());
    settle(&mut rt);
    assert!(has(&rt, keys::MCP_ADD_FORM));
    assert!(has(&rt, keys::MCP_ADD_ERROR));
    assert!(servers.added.lock().unwrap().is_empty());
}
