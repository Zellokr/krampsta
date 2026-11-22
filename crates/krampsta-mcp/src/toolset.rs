//! [`ToolSet`]: the [`Toolbox`] the agent gets, the project's own tools and the MCP servers'
//! together. Calls to a server's tools follow the user's tool rules in `permissions.json`,
//! through the local toolbox (the project's `Workspace`, or `Permissions` when no folder is
//! open): without a rule, they ask.

use std::sync::{Arc, Mutex, PoisonError};

use krampsta_agent::{Access, Approval, Call, ProjectFiles, ToolOutput, ToolServers, ToolSpec, Toolbox};

use crate::McpServers;
use crate::servers::PREFIX;

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

    fn guidance(&self) -> Option<Arc<str>> {
        self.local.as_ref().and_then(|l| l.guidance())
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

    fn files(&self) -> Result<ProjectFiles, String> {
        match &self.local {
            Some(local) => local.files(),
            None => Err("no project folder is open".into()),
        }
    }

    fn set_pending(&self, path: &str, content: Option<Arc<str>>) {
        if let Some(local) = &self.local {
            local.set_pending(path, content);
        }
    }
}
