//! The configured servers while Krampsta runs.
//!
//! [`McpServers::start`] connects every enabled server in the background, on a small tokio
//! runtime of its own; each server's tools join the registry ([`ToolSet`](crate::ToolSet)) once
//! it is ready, as `mcp__<server>__<tool>`, for the runs that start after. Settings adds,
//! removes, starts and stops servers while it runs ([`ToolServers`]): the change applies at
//! once and `mcp.json` follows in the background, one edit after another (`edits.rs`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use krampsta_agent::{Access, NewServer, Notifier, ServerState, ServerStatus, ToolOutput, ToolServers, ToolSpec};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::ServerConfig;
use crate::edits::{self, Edit};
use crate::{McpClient, McpError, McpTool, Transport};

/// What a tool's name starts with when a server offers it.
pub const PREFIX: &str = "mcp__";
/// The longest tool name providers accept.
const MAX_NAME: usize = 64;

/// Where a server's connection is (what the UI sees is [`ServerState`], without the client).
#[derive(Clone, Debug)]
enum Connection {
    Connecting,
    Ready { client: Arc<McpClient>, tools: Vec<McpTool> },
    Failed(String),
    Disabled,
}

#[derive(Debug)]
struct Server {
    config: ServerConfig,
    state: Connection,
    /// Which connection attempt is current: one that finishes after the server was stopped,
    /// started again or removed is dropped.
    attempt: u64,
}

/// What the background work and the callers share.
#[derive(Default)]
struct Shared {
    servers: RwLock<Vec<Server>>,
    /// Grows whenever a server's state changes, so the registry and the UI know to look again.
    version: AtomicU64,
    /// Who to wake when it does (set once the window exists).
    wake: Mutex<Option<Notifier>>,
    /// Why the last edit to `mcp.json` failed, until the UI takes it.
    problem: Mutex<Option<String>>,
    attempts: AtomicU64,
}

impl Shared {
    fn changed(&self) {
        self.version.fetch_add(1, Ordering::Release);
        let notify = self.wake.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(notify) = notify {
            notify();
        }
    }

    fn servers(&self) -> std::sync::RwLockReadGuard<'_, Vec<Server>> {
        self.servers.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn servers_mut(&self) -> std::sync::RwLockWriteGuard<'_, Vec<Server>> {
        self.servers.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The configured servers and their connections. Dropping it closes them (and stops the stdio
/// servers' processes).
pub struct McpServers {
    runtime: Option<tokio::runtime::Runtime>,
    shared: Arc<Shared>,
    file: Option<PathBuf>,
    /// Edits for `mcp.json`, written in order by one task. None: no file, nothing saved.
    edits: Option<mpsc::UnboundedSender<Edit>>,
}

impl std::fmt::Debug for McpServers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServers").field("servers", &self.statuses()).finish()
    }
}

impl McpServers {
    /// Starts connecting every enabled server in `configs` (read from `file`, where changes are
    /// saved), in the background.
    pub fn start(configs: Vec<ServerConfig>, file: Option<PathBuf>) -> std::io::Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("krampsta-mcp")
            .enable_all()
            .build()?;
        let shared = Arc::new(Shared::default());
        let edits = file.clone().map(|file| {
            // Unbounded: the UI thread never waits to queue, and each edit is one click.
            let (edits, inbox) = mpsc::unbounded_channel();
            runtime.spawn(write_edits(file, inbox, shared.clone()));
            edits
        });
        let servers = McpServers { runtime: Some(runtime), shared, file, edits };
        for config in configs {
            let start = (!config.disabled).then(|| config.name.clone());
            servers.shared.servers_mut().push(Server { config, state: Connection::Disabled, attempt: 0 });
            if let Some(name) = start {
                servers.connect(&name);
            }
        }
        Ok(Arc::new(servers))
    }

    /// No servers (tests).
    pub fn none() -> std::io::Result<Arc<Self>> {
        Self::start(Vec::new(), None)
    }

    /// Starts connecting `name` in the background (a fresh attempt).
    fn connect(&self, name: &str) {
        let Some(runtime) = &self.runtime else { return };
        let attempt = self.shared.attempts.fetch_add(1, Ordering::Relaxed) + 1;
        let transport = {
            let mut servers = self.shared.servers_mut();
            let Some(server) = servers.iter_mut().find(|s| s.config.name == name) else { return };
            server.state = Connection::Connecting;
            server.attempt = attempt;
            server.config.transport.clone()
        };
        let (shared, name) = (self.shared.clone(), name.to_owned());
        runtime.spawn(async move {
            let state = match connect(&transport).await {
                Ok((client, tools)) => Connection::Ready { client: Arc::new(client), tools },
                Err(e) => Connection::Failed(e.to_string()),
            };
            let mut servers = shared.servers_mut();
            match servers.iter_mut().find(|s| s.config.name == name && s.attempt == attempt) {
                Some(server) => server.state = state,
                // Stopped, started again or removed meanwhile: this attempt no longer counts.
                None => return,
            }
            drop(servers);
            shared.changed();
        });
    }

    fn save(&self, edit: Edit) {
        if let Some(edits) = &self.edits {
            // The writer only stops with the runtime, when nothing is left to save.
            let _ = edits.send(edit);
        }
    }

    /// The ready servers' tools, named as the model calls them.
    pub(crate) fn specs(&self) -> Vec<ToolSpec> {
        let servers = self.shared.servers();
        let mut seen = HashSet::new();
        let mut specs = Vec::new();
        for server in servers.iter() {
            let Connection::Ready { tools, .. } = &server.state else { continue };
            for tool in tools {
                let name = tool_name(&server.config.name, &tool.name);
                // Two tools whose names only differ past the cut, or by what was replaced:
                // the first one keeps the name.
                if seen.insert(name.clone()) {
                    let description = format!("{} (from the MCP server `{}`)", tool.description, server.config.name);
                    specs.push(ToolSpec { name, description, parameters: tool.input_schema.clone() });
                }
            }
        }
        specs
    }

    /// The server and its tool behind `name`.
    fn find(&self, name: &str) -> Option<(String, Arc<McpClient>, String)> {
        let servers = self.shared.servers();
        servers.iter().find_map(|server| {
            let Connection::Ready { client, tools } = &server.state else { return None };
            let tool = tools.iter().find(|t| tool_name(&server.config.name, &t.name) == name)?;
            Some((server.config.name.clone(), client.clone(), tool.name.clone()))
        })
    }

    /// Calls `name` and waits for its reply on this (blocking) thread.
    pub(crate) fn call(&self, name: &str, arguments: &str, cancel: &CancellationToken) -> Result<ToolOutput, String> {
        let (server, client, tool) =
            self.find(name).ok_or_else(|| format!("`{name}` isn't available: its MCP server isn't connected"))?;
        let runtime = self.runtime.as_ref().ok_or("the MCP servers have closed")?;
        let (reply, answer) = std::sync::mpsc::sync_channel(1);
        let (arguments, cancel) = (arguments.to_owned(), cancel.clone());
        runtime.spawn(async move {
            // The receiver is gone only if the call was abandoned: nobody is left to tell.
            let _ = reply.send(client.call(&tool, &arguments, &cancel).await);
        });
        match answer.recv() {
            Ok(Ok(reply)) if reply.is_error => Err(reply.text),
            Ok(Ok(reply)) => Ok(reply.text.into()),
            Ok(Err(McpError::Cancelled)) => Err("stopped".into()),
            Ok(Err(e)) => Err(format!("{server}: {e}")),
            Err(_) => Err(format!("the connection to `{server}` closed")),
        }
    }

    /// What the user is asked about a call: the server, the tool and its arguments.
    pub(crate) fn ask(&self, name: &str, arguments: &str) -> Access {
        let shown = match self.find(name) {
            Some((server, _, tool)) => format!("{tool} on the MCP server `{server}`"),
            None => name.to_owned(),
        };
        let arguments = serde_json::from_str::<serde_json::Value>(arguments)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| arguments.to_owned());
        Access::Ask { what: format!("{shown}\n{arguments}") }
    }
}

impl ToolServers for McpServers {
    fn version(&self) -> u64 {
        self.shared.version.load(Ordering::Acquire)
    }

    fn statuses(&self) -> Vec<ServerStatus> {
        self.shared
            .servers()
            .iter()
            .map(|s| ServerStatus {
                name: s.config.name.clone(),
                reached: reached(&s.config.transport),
                state: match &s.state {
                    Connection::Connecting => ServerState::Connecting,
                    Connection::Ready { tools, .. } => {
                        ServerState::Ready { tools: tools.iter().map(|t| t.name.clone()).collect() }
                    }
                    Connection::Failed(why) => ServerState::Failed(why.clone()),
                    Connection::Disabled => ServerState::Disabled,
                },
            })
            .collect()
    }

    fn config_file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    fn set_notifier(&self, notify: Notifier) {
        *self.shared.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(notify);
    }

    fn add(&self, new: NewServer) -> Result<(), String> {
        let config = {
            let servers = self.shared.servers();
            edits::server_config(&new, |name| servers.iter().any(|s| s.config.name == name))?
        };
        let name = config.name.clone();
        self.shared.servers_mut().push(Server { config: config.clone(), state: Connection::Disabled, attempt: 0 });
        self.connect(&name);
        self.save(Edit::Add(config));
        self.shared.changed();
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<(), String> {
        let mut servers = self.shared.servers_mut();
        let index =
            servers.iter().position(|s| s.config.name == name).ok_or_else(|| format!("there is no server `{name}`"))?;
        // Its client goes with it: a stdio server's process stops once no call holds it.
        servers.remove(index);
        drop(servers);
        self.save(Edit::Remove(name.to_owned()));
        self.shared.changed();
        Ok(())
    }

    fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        {
            let mut servers = self.shared.servers_mut();
            let server = servers
                .iter_mut()
                .find(|s| s.config.name == name)
                .ok_or_else(|| format!("there is no server `{name}`"))?;
            if server.config.disabled == !enabled {
                return Ok(());
            }
            server.config.disabled = !enabled;
            if !enabled {
                server.state = Connection::Disabled;
                server.attempt = 0;
            }
        }
        if enabled {
            self.connect(name);
        }
        self.save(Edit::Enabled(name.to_owned(), enabled));
        self.shared.changed();
        Ok(())
    }

    fn take_problem(&self) -> Option<String> {
        self.shared.problem.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// Writes the edits to `file` one after another, off the async workers. A failure is kept for
/// the UI to show.
async fn write_edits(file: PathBuf, mut inbox: mpsc::UnboundedReceiver<Edit>, shared: Arc<Shared>) {
    while let Some(edit) = inbox.recv().await {
        let file = file.clone();
        let written = tokio::task::spawn_blocking(move || edits::apply(&file, &edit))
            .await
            .unwrap_or_else(|e| Err(format!("saving the MCP servers failed: {e}")));
        if let Err(why) = written {
            *shared.problem.lock().unwrap_or_else(PoisonError::into_inner) = Some(why);
            shared.changed();
        }
    }
}

/// How a server is reached, in a line: its command and arguments, or its URL. Never its env
/// values or headers (they hold tokens).
fn reached(transport: &Transport) -> String {
    match transport {
        Transport::Stdio { command, args, .. } => {
            std::iter::once(command.as_str()).chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
        }
        Transport::Http { url, .. } => url.clone(),
    }
}

impl Drop for McpServers {
    fn drop(&mut self) {
        // Safe from any thread, async or not: the connections close in the background.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

async fn connect(transport: &Transport) -> Result<(McpClient, Vec<McpTool>), McpError> {
    let client = McpClient::connect(transport).await?;
    let tools = client.tools().await?;
    Ok((client, tools))
}

/// `mcp__<server>__<tool>`, with anything a provider rejects replaced by `_`, cut to
/// [`MAX_NAME`] characters.
pub fn tool_name(server: &str, tool: &str) -> String {
    let name: String = format!("{PREFIX}{server}__{tool}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .take(MAX_NAME)
        .collect();
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_what_providers_accept() {
        assert_eq!(tool_name("github", "create_issue"), "mcp__github__create_issue");
        assert_eq!(tool_name("docs", "search.v2 (beta)"), "mcp__docs__search_v2__beta_");
        assert_eq!(tool_name("s", &"x".repeat(100)).len(), MAX_NAME);
    }
}
