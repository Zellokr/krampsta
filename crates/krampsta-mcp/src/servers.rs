//! The configured servers while Krampsta runs, and their tools in the agent's registry.
//!
//! [`McpServers::start`] connects every enabled server in the background, on a small tokio
//! runtime of its own; each server's tools join the registry once it is ready, as
//! `mcp__<server>__<tool>`, for the runs that start after. [`ToolSet`] is the [`Toolbox`] the
//! agent gets: the project's own tools and the servers' together. Calls to a server's tools
//! follow the user's tool rules in `permissions.json`, through the local toolbox (the
//! project's `Workspace`, or `Permissions` when no folder is open): without a rule, they ask.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use krampsta_agent::{Access, Approval, Call, ToolOutput, ToolSpec, Toolbox};
use tokio_util::sync::CancellationToken;

use crate::config::ServerConfig;
use crate::{McpClient, McpError, McpTool};

/// What a tool's name starts with when a server offers it.
pub const PREFIX: &str = "mcp__";
/// The longest tool name providers accept.
const MAX_NAME: usize = 64;

/// Where a server is.
#[derive(Clone, Debug)]
pub enum ServerState {
    Connecting,
    Ready { client: Arc<McpClient>, tools: Vec<McpTool> },
    Failed(String),
    Disabled,
}

#[derive(Debug)]
struct Server {
    name: String,
    state: ServerState,
}

/// The configured servers and their connections. Dropping it closes them (and stops the stdio
/// servers' processes).
pub struct McpServers {
    runtime: Option<tokio::runtime::Runtime>,
    servers: Arc<RwLock<Vec<Server>>>,
    /// Grows whenever a server's state changes, so the registry knows to rebuild.
    version: Arc<AtomicU64>,
}

impl std::fmt::Debug for McpServers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServers").field("servers", &self.statuses()).finish()
    }
}

impl McpServers {
    /// Starts connecting every enabled server in `configs`, in the background. `changed` is
    /// called whenever one is ready or fails.
    pub fn start(configs: Vec<ServerConfig>, changed: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("krampsta-mcp")
            .enable_all()
            .build()?;
        let servers: Vec<Server> = configs
            .iter()
            .map(|c| Server {
                name: c.name.clone(),
                state: if c.disabled { ServerState::Disabled } else { ServerState::Connecting },
            })
            .collect();
        let servers = Arc::new(RwLock::new(servers));
        let version = Arc::new(AtomicU64::new(0));
        for config in configs.into_iter().filter(|c| !c.disabled) {
            let (servers, version, changed) = (servers.clone(), version.clone(), changed.clone());
            runtime.spawn(async move {
                let state = match connect(&config).await {
                    Ok((client, tools)) => ServerState::Ready { client: Arc::new(client), tools },
                    Err(e) => ServerState::Failed(e.to_string()),
                };
                let mut servers = servers.write().unwrap_or_else(PoisonError::into_inner);
                if let Some(server) = servers.iter_mut().find(|s| s.name == config.name) {
                    server.state = state;
                }
                drop(servers);
                version.fetch_add(1, Ordering::Release);
                changed();
            });
        }
        Ok(Arc::new(McpServers { runtime: Some(runtime), servers, version }))
    }

    /// No servers (headless runs, tests).
    pub fn none() -> std::io::Result<Arc<Self>> {
        Self::start(Vec::new(), Arc::new(|| {}))
    }

    /// Every server and where it is, in the order configured.
    pub fn statuses(&self) -> Vec<(String, ServerState)> {
        let servers = self.servers.read().unwrap_or_else(PoisonError::into_inner);
        servers.iter().map(|s| (s.name.clone(), s.state.clone())).collect()
    }

    fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// The ready servers' tools, named as the model calls them.
    fn specs(&self) -> Vec<ToolSpec> {
        let servers = self.servers.read().unwrap_or_else(PoisonError::into_inner);
        let mut seen = HashSet::new();
        let mut specs = Vec::new();
        for server in servers.iter() {
            let ServerState::Ready { tools, .. } = &server.state else { continue };
            for tool in tools {
                let name = tool_name(&server.name, &tool.name);
                // Two tools whose names only differ past the cut, or by what was replaced:
                // the first one keeps the name.
                if seen.insert(name.clone()) {
                    let description = format!("{} (from the MCP server `{}`)", tool.description, server.name);
                    specs.push(ToolSpec { name, description, parameters: tool.input_schema.clone() });
                }
            }
        }
        specs
    }

    /// The server and its tool behind `name`.
    fn find(&self, name: &str) -> Option<(String, Arc<McpClient>, String)> {
        let servers = self.servers.read().unwrap_or_else(PoisonError::into_inner);
        servers.iter().find_map(|server| {
            let ServerState::Ready { client, tools } = &server.state else { return None };
            let tool = tools.iter().find(|t| tool_name(&server.name, &t.name) == name)?;
            Some((server.name.clone(), client.clone(), tool.name.clone()))
        })
    }

    /// Calls `name` and waits for its reply on this (blocking) thread.
    fn call(&self, name: &str, arguments: &str, cancel: &CancellationToken) -> Result<ToolOutput, String> {
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
    fn ask(&self, name: &str, arguments: &str) -> Access {
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

impl Drop for McpServers {
    fn drop(&mut self) {
        // Safe from any thread, async or not: the connections close in the background.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

async fn connect(config: &ServerConfig) -> Result<(McpClient, Vec<McpTool>), McpError> {
    let client = McpClient::connect(&config.transport).await?;
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

/// The agent's tools: the project's own (if a folder is open) and the MCP servers'.
pub struct ToolSet {
    local: Option<Arc<dyn Toolbox>>,
    mcp: Arc<McpServers>,
    /// The specs last built, and the servers' version they were built at.
    specs: Mutex<(u64, Arc<[ToolSpec]>)>,
}

impl ToolSet {
    pub fn new(local: Option<Arc<dyn Toolbox>>, mcp: Arc<McpServers>) -> Self {
        let specs = Mutex::new((u64::MAX, Arc::from([])));
        ToolSet { local, mcp, specs }
    }
}

impl Toolbox for ToolSet {
    fn specs(&self) -> Arc<[ToolSpec]> {
        let version = self.mcp.version();
        let mut cached = self.specs.lock().unwrap_or_else(PoisonError::into_inner);
        if cached.0 != version {
            let local = self.local.as_ref().map(|t| t.specs());
            let all: Vec<ToolSpec> = local.iter().flat_map(|s| s.iter().cloned()).chain(self.mcp.specs()).collect();
            *cached = (version, all.into());
        }
        cached.1.clone()
    }

    fn run(&self, name: &str, arguments: &str, call: &Call<'_>) -> Result<ToolOutput, String> {
        if name.starts_with(PREFIX) {
            return self.mcp.call(name, arguments, call.cancel);
        }
        match &self.local {
            Some(local) => local.run(name, arguments, call),
            None => Err(format!("`{name}` needs a project folder: open one first")),
        }
    }

    /// The user's rules decide (the local toolbox holds them); a call that asks shows its
    /// server, tool and arguments. Without rules, every MCP call asks.
    fn check(&self, name: &str, arguments: &str) -> Access {
        let access = match &self.local {
            Some(local) => local.check(name, arguments),
            None if name.starts_with(PREFIX) => Access::Ask { what: name.to_owned() },
            None => Access::Allow(Approval::NotNeeded),
        };
        match access {
            // A server's tool always needs permission: a toolbox that doesn't know it (the
            // trait's default) never lets it through unasked.
            Access::Ask { .. } | Access::Allow(Approval::NotNeeded) if name.starts_with(PREFIX) => {
                self.mcp.ask(name, arguments)
            }
            access => access,
        }
    }

    fn remember(&self, name: &str, arguments: &str) -> Result<(), String> {
        match &self.local {
            Some(local) => local.remember(name, arguments),
            None => Err("there are no rules to keep it in".into()),
        }
    }

    fn write(&self, path: &str, expected: Option<&str>, content: &str) -> Result<(), String> {
        match &self.local {
            Some(local) => local.write(path, expected, content),
            None => Err(format!("`{path}` can't be written: no project folder is open")),
        }
    }

    fn set_pending(&self, path: &str, content: Option<Arc<str>>) {
        if let Some(local) = &self.local {
            local.set_pending(path, content);
        }
    }
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
