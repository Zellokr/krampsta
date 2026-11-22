//! The MCP servers as the window shows them: each one's state in Settings (connecting, its
//! tools, why it failed, disabled), from the [`ToolServers`] the app hands over. The list is
//! copied only when its version moves (in `pump`, woken by the servers), never per frame. A
//! server that fails says so in the composer's notice; the reason is in Settings.

use std::sync::Arc;

use krampsta_agent::{Notifier, ServerState, ServerStatus, ToolServers};
use rune::{El, col, el, row, text};
use rune_kit::{Kit, Spinner};

use crate::state::KrampstaApp;
use crate::{keys, theme};

/// The servers, and the list as last read.
#[derive(Default)]
pub struct ServerList {
    source: Option<Arc<dyn ToolServers>>,
    /// Every server and where it is, as last read.
    pub list: Vec<ServerStatus>,
    seen: u64,
}

impl KrampstaApp {
    /// Shows `servers` in Settings, and keeps their state up to date.
    pub fn with_servers(mut self, servers: Arc<dyn ToolServers>) -> Self {
        self.show_servers(servers);
        self
    }

    /// `with_servers` for a scene, which has the app by reference.
    pub(crate) fn show_servers(&mut self, servers: Arc<dyn ToolServers>) {
        self.servers = ServerList { list: servers.statuses(), seen: servers.version(), source: Some(servers) };
    }

    pub(crate) fn set_servers_notifier(&self, notify: Notifier) {
        if let Some(source) = &self.servers.source {
            source.set_notifier(notify);
        }
    }

    /// Reads the servers again if any changed. True if so.
    pub(crate) fn land_servers(&mut self) -> bool {
        let Some(source) = &self.servers.source else { return false };
        let version = source.version();
        if version == self.servers.seen {
            return false;
        }
        let fresh = source.statuses();
        let failed = |s: &ServerStatus| matches!(s.state, ServerState::Failed(_));
        let newly =
            fresh.iter().find(|s| failed(s) && !self.servers.list.iter().any(|o| o.name == s.name && failed(o)));
        if let Some(server) = newly {
            self.notice = Some(format!("The MCP server `{}` couldn't start. Settings says why.", server.name));
        }
        self.servers.list = fresh;
        self.servers.seen = version;
        true
    }
}

/// The "MCP servers" part of Settings, when there are servers to show (or a file to add them to).
pub fn settings(s: &KrampstaApp, k: &Kit) -> Option<El> {
    let source = s.servers.source.as_ref()?;
    let file = source.config_file().map(crate::projects::display_path);
    let mut part = col().w_full().gap(12.0).child(text("MCP servers").font(k.ty.small_medium).color(k.c.text_faint));
    let note = match &file {
        Some(file) => format!("From {file}, in the format Claude Desktop uses. Krampsta connects them when it starts."),
        None => "Krampsta connects them when it starts.".to_owned(),
    };
    part = part.child(text(note).font(k.ty.small).color(k.c.text_muted).wrap().selectable(true));
    if s.servers.list.is_empty() {
        let empty = card(k)
            .key(keys::MCP_SERVERS_EMPTY)
            .child(text("No servers yet").font(k.ty.ui_medium).color(k.c.text_base))
            .child(
                text("Add them under \"mcpServers\" in that file, then start Krampsta again.")
                    .font(k.ty.small)
                    .color(k.c.text_muted)
                    .wrap(),
            );
        return Some(part.child(empty));
    }
    Some(part.children(s.servers.list.iter().map(|server| server_card(k, server))))
}

fn card(k: &Kit) -> El {
    col().w_full().py(12.0).px(16.0).gap(4.0).radius(k.radius.md).fill(k.c.layer_01).border(1.0, k.c.border_weak)
}

/// One server: its name and how it is reached, its state, and its tools or why it failed.
fn server_card(k: &Kit, server: &ServerStatus) -> El {
    let head = row()
        .w_full()
        .gap(16.0)
        .items_center()
        .justify_between()
        .child(
            col()
                .gap(2.0)
                .shrink()
                .min_w(0.0)
                .child(text(server.name.clone()).font(k.ty.ui_medium).color(k.c.text_base))
                .child(text(server.reached.clone()).font(k.ty.small).color(k.c.text_faint).truncate()),
        )
        .child(state(k, &server.state).key(keys::mcp_server_state(&server.name)));
    let detail = match &server.state {
        ServerState::Ready { tools } if !tools.is_empty() => {
            Some(text(tools.join(", ")).font(k.ty.small).color(k.c.text_muted).wrap().selectable(true))
        }
        ServerState::Failed(why) => {
            Some(text(why.clone()).font(k.ty.small).color(k.c.danger.icon).wrap().selectable(true))
        }
        _ => None,
    };
    card(k).key(keys::mcp_server(&server.name)).child(head).children(detail.map(|d| d.mt(6.0)))
}

/// Where a server is, as a dot (or a spinner) and a word.
fn state(k: &Kit, state: &ServerState) -> El {
    let dot = |color| el().size(8.0, 8.0).radius(4.0).fill(color);
    let (mark, label) = match state {
        ServerState::Connecting => {
            (Spinner::new().size(12.0).color(theme::accent(k.dark)).build(k), "Connecting…".to_owned())
        }
        ServerState::Ready { tools } => {
            let label = match tools.len() {
                1 => "Connected · 1 tool".to_owned(),
                n => format!("Connected · {n} tools"),
            };
            (dot(k.c.success.icon), label)
        }
        ServerState::Failed(_) => (dot(k.c.danger.icon), "Failed".to_owned()),
        ServerState::Disabled => (dot(k.c.text_faint), "Disabled".to_owned()),
    };
    row().gap(6.0).items_center().child(mark).child(text(label).font(k.ty.small).color(k.c.text_muted))
}

/// The `mcp-servers` scene: Settings with a server in each state.
pub(crate) fn scene(s: &mut KrampstaApp) {
    let server = |name: &str, reached: &str, state| ServerStatus { name: name.into(), reached: reached.into(), state };
    let tools = ["create_issue", "get_issue", "list_pull_requests", "search_code"];
    let statuses = vec![
        server(
            "github",
            "npx -y @modelcontextprotocol/server-github",
            ServerState::Ready { tools: tools.iter().map(|t| (*t).to_owned()).collect() },
        ),
        server("docs", "https://docs.example.com/mcp", ServerState::Connecting),
        server(
            "postgres",
            "npx -y @modelcontextprotocol/server-postgres",
            ServerState::Failed(
                "the MCP server couldn't be reached: connection closed: initialize response. It said: \
                 error: DATABASE_URL is not set"
                    .into(),
            ),
        ),
        server("old-search", "old-search --stdio", ServerState::Disabled),
    ];
    let file = Some("/Users/you/.config/krampsta/mcp.json".into());
    s.show_servers(Arc::new(krampsta_agent::FixedServers::new(statuses, file)));
    s.screen = crate::state::Screen::Settings;
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use krampsta_agent::{FixedServers, MockCatalog, MockConfig};

    use super::*;

    /// Servers whose state the test moves, as connections finishing would.
    #[derive(Default)]
    struct Moving {
        list: Mutex<Vec<ServerStatus>>,
        version: AtomicU64,
    }

    impl Moving {
        fn set(&self, state: ServerState) {
            *self.list.lock().unwrap() =
                vec![ServerStatus { name: "docs".into(), reached: "docs-server".into(), state }];
            self.version.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl ToolServers for Moving {
        fn version(&self) -> u64 {
            self.version.load(Ordering::SeqCst)
        }
        fn statuses(&self) -> Vec<ServerStatus> {
            self.list.lock().unwrap().clone()
        }
        fn config_file(&self) -> Option<&Path> {
            None
        }
        fn set_notifier(&self, _: Notifier) {}
    }

    fn app() -> KrampstaApp {
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())))
    }

    #[test]
    fn the_list_is_read_again_only_when_it_changes_and_a_failure_says_so() {
        let servers = Arc::new(Moving::default());
        servers.set(ServerState::Connecting);
        let mut app = app().with_servers(servers.clone());
        assert_eq!(app.servers.list[0].state, ServerState::Connecting);
        assert!(!app.land_servers(), "nothing moved");

        servers.set(ServerState::Failed("boom".into()));
        assert!(app.land_servers());
        assert!(app.notice.as_deref().is_some_and(|n| n.contains("`docs` couldn't start")), "{:?}", app.notice);
        app.notice = None;
        servers.set(ServerState::Failed("boom again".into()));
        assert!(app.land_servers());
        assert_eq!(app.notice, None, "a server already failed isn't announced twice");
        assert_eq!(app.servers.list[0].state, ServerState::Failed("boom again".into()));
    }

    #[test]
    fn the_window_hands_its_waker_to_the_servers() {
        let servers = Arc::new(FixedServers::new(Vec::new(), None));
        let mut app = app().with_servers(servers.clone());
        app.set_notifier(Arc::new(|| {}));
        assert!(servers.has_notifier());
    }
}
