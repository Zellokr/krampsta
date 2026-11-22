//! The MCP servers as the window keeps them: the list Settings shows (from the [`ToolServers`]
//! the app hands over), the form that adds one, and which one is waiting to be removed. The list
//! is copied only when its version moves (in `pump`, woken by the servers), never per frame. A
//! server that fails says so in the composer's notice, and so does a change that can't be saved.
//! The Settings part is drawn by `views/mcp.rs`.

use std::sync::Arc;

use krampsta_agent::{NewServer, Notifier, ServerKind, ServerState, ServerStatus, ToolServers};
use rune::TextEdit;

use crate::state::KrampstaApp;

/// The servers, the list as last read, and what Settings is doing with them.
#[derive(Default)]
pub struct ServerList {
    pub(crate) source: Option<Arc<dyn ToolServers>>,
    /// Every server and where it is, as last read.
    pub list: Vec<ServerStatus>,
    seen: u64,
    /// The "Add a server" form.
    pub form: ServerForm,
    /// The server whose removal waits for a second click.
    pub confirm_remove: Option<String>,
}

/// The form that adds a server. Closed until "Add a server" opens it.
#[derive(Default)]
pub struct ServerForm {
    pub open: bool,
    pub name: TextEdit,
    /// Started by a command (stdio), or reached at a URL.
    pub url: bool,
    /// The command line, or the URL.
    pub target: TextEdit,
    /// `KEY=value` (or `Name: value`) lines.
    pub extra: TextEdit,
    /// Why the last "Add" didn't go through.
    pub error: Option<String>,
}

impl KrampstaApp {
    /// Shows `servers` in Settings, and keeps their state up to date.
    pub fn with_servers(mut self, servers: Arc<dyn ToolServers>) -> Self {
        self.show_servers(servers);
        self
    }

    /// `with_servers` for a scene, which has the app by reference.
    pub(crate) fn show_servers(&mut self, servers: Arc<dyn ToolServers>) {
        self.servers.list = servers.statuses();
        self.servers.seen = servers.version();
        self.servers.source = Some(servers);
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
        if let Some(problem) = source.take_problem() {
            self.notice = Some(problem);
        }
        self.servers.list = fresh;
        self.servers.seen = version;
        true
    }

    pub fn open_server_form(&mut self) {
        self.servers.form = ServerForm { open: true, ..ServerForm::default() };
        self.servers.form.name.focused = true;
    }

    pub fn close_server_form(&mut self) {
        self.servers.form = ServerForm::default();
    }

    /// Adds the server the form describes. If the servers refuse it, the form says why.
    pub fn add_server(&mut self) {
        let Some(source) = &self.servers.source else { return };
        let form = &self.servers.form;
        let new = NewServer {
            name: form.name.text.clone(),
            kind: if form.url { ServerKind::Url } else { ServerKind::Command },
            target: form.target.text.clone(),
            extra: form.extra.text.clone(),
        };
        match source.add(new) {
            Ok(()) => {
                self.close_server_form();
                self.land_servers();
            }
            Err(why) => self.servers.form.error = Some(why),
        }
    }

    /// Starts or stops a server.
    pub fn toggle_server(&mut self, name: &str) {
        let Some(source) = &self.servers.source else { return };
        let enabled = self.servers.list.iter().any(|s| s.name == name && s.state == ServerState::Disabled);
        if let Err(why) = source.set_enabled(name, enabled) {
            self.notice = Some(why);
        }
        self.land_servers();
    }

    /// The first click on "Remove": the button asks to be clicked again.
    pub fn ask_remove_server(&mut self, name: &str) {
        self.servers.confirm_remove = Some(name.to_owned());
    }

    pub fn cancel_remove_server(&mut self) {
        self.servers.confirm_remove = None;
    }

    /// Removes the server waiting for confirmation.
    pub fn confirm_remove_server(&mut self) {
        let Some(name) = self.servers.confirm_remove.take() else { return };
        let Some(source) = &self.servers.source else { return };
        if let Err(why) = source.remove(&name) {
            self.notice = Some(why);
        }
        self.land_servers();
    }
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

/// The `mcp-add` scene: the form filled in for a server at a URL, and a server waiting for the
/// second click that removes it.
pub(crate) fn scene_add(s: &mut KrampstaApp) {
    let github = ServerStatus {
        name: "github".into(),
        reached: "npx -y @modelcontextprotocol/server-github".into(),
        state: ServerState::Ready { tools: vec!["create_issue".into(), "get_issue".into()] },
    };
    let file = Some("/Users/you/.config/krampsta/mcp.json".into());
    s.show_servers(Arc::new(krampsta_agent::FixedServers::new(vec![github], file)));
    s.servers.confirm_remove = Some("github".into());
    s.servers.form = ServerForm {
        open: true,
        name: TextEdit::new("docs"),
        url: true,
        target: TextEdit::new("https://docs.example.com/mcp"),
        extra: TextEdit::new("Authorization: Bearer …"),
        error: None,
    };
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
