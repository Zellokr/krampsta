//! The user's permission rules while Krampsta runs: read once from `permissions.json` (the
//! config folder) and saved again when the user allows something "always".
//!
//! A [`Workspace`](crate::Workspace) checks its commands with them. Tools that come from
//! elsewhere (an MCP server's) follow the tool rules: through the workspace when a folder is
//! open, and through [`Permissions`] itself, a [`Toolbox`] with no tools of its own, when none is.

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use krampsta_agent::{Access, Call, ToolOutput, ToolSpec, Toolbox};

use crate::Policy;

pub struct Permissions {
    policy: RwLock<Policy>,
    /// Krampsta's config folder, where `permissions.json` lives. None: rules can't be saved.
    config: Option<PathBuf>,
    /// The file exists but couldn't be read: saving over it would lose the user's rules.
    broken: bool,
}

impl Permissions {
    /// The rules in the user's config folder.
    pub fn open() -> Self {
        Self::open_with(dirs::config_dir().map(|d| d.join("krampsta")))
    }

    /// The rules in `config` (tests pass a temporary folder; None: no rules, none saved).
    pub fn open_with(config: Option<PathBuf>) -> Self {
        let (policy, broken) = match config.as_deref().map(|c| Policy::load(&c.join("permissions.json"))) {
            Some(Ok(policy)) => (policy, false),
            Some(Err(e)) => {
                // With no rules everything asks: safe, if tedious, until the file is fixed.
                eprintln!("krampsta: {e}");
                (Policy::default(), true)
            }
            None => (Policy::default(), false),
        };
        Permissions { policy: RwLock::new(policy), config, broken }
    }

    pub(crate) fn config(&self) -> Option<&Path> {
        self.config.as_deref()
    }

    pub(crate) fn check_command(&self, command: &str) -> Access {
        self.policy.read().unwrap_or_else(PoisonError::into_inner).check(command)
    }

    pub(crate) fn check_tool(&self, name: &str) -> Access {
        self.policy.read().unwrap_or_else(PoisonError::into_inner).check_tool(name)
    }

    pub(crate) fn sandbox(&self) -> crate::sandbox::SandboxRules {
        self.policy.read().unwrap_or_else(PoisonError::into_inner).sandbox.clone()
    }

    pub(crate) fn allow_command(&self, command: &str) -> Result<(), String> {
        self.change(|policy| policy.allow(command))
    }

    pub(crate) fn allow_tool(&self, name: &str) -> Result<(), String> {
        self.change(|policy| policy.allow_tool(name))
    }

    /// Changes the rules and saves them.
    fn change(&self, edit: impl FnOnce(&mut Policy)) -> Result<(), String> {
        let config = self.config.as_ref().ok_or("there is no config folder to keep the rule in")?;
        if self.broken {
            return Err("permissions.json isn't valid: fix it before Krampsta saves rules in it".into());
        }
        let rules = {
            let mut policy = self.policy.write().unwrap_or_else(PoisonError::into_inner);
            edit(&mut policy);
            policy.clone()
        };
        rules.save(&config.join("permissions.json"))
    }
}

impl Toolbox for Permissions {
    fn specs(&self) -> Arc<[ToolSpec]> {
        Arc::from([])
    }

    fn run(&self, name: &str, _: &str, _: &Call<'_>) -> Result<ToolOutput, String> {
        Err(format!("`{name}` needs a project folder: open one first"))
    }

    fn check(&self, name: &str, _: &str) -> Access {
        self.check_tool(name)
    }

    fn remember(&self, name: &str, _: &str) -> Result<(), String> {
        self.allow_tool(name)
    }
}

#[cfg(test)]
mod tests {
    use krampsta_agent::Approval;

    use super::*;

    #[test]
    fn without_a_folder_tools_follow_the_saved_rules() {
        let config = tempfile::tempdir().unwrap();
        let rules = Permissions::open_with(Some(config.path().to_owned()));
        assert!(rules.specs().is_empty());
        assert!(matches!(rules.check("mcp__docs__search", "{}"), Access::Ask { .. }));
        rules.remember("mcp__docs__search", "{}").unwrap();
        assert_eq!(rules.check("mcp__docs__search", "{}"), Access::Allow(Approval::Rule("mcp__docs__search".into())));

        let reopened = Permissions::open_with(Some(config.path().to_owned()));
        assert!(matches!(reopened.check("mcp__docs__search", "{}"), Access::Allow(_)), "saved between launches");
    }

    #[test]
    fn a_broken_file_is_never_saved_over() {
        let config = tempfile::tempdir().unwrap();
        let file = config.path().join("permissions.json");
        std::fs::write(&file, "{ broken").unwrap();
        let rules = Permissions::open_with(Some(config.path().to_owned()));
        assert!(matches!(rules.check("mcp__x__y", "{}"), Access::Ask { .. }), "everything asks meanwhile");
        assert!(rules.remember("mcp__x__y", "{}").unwrap_err().contains("isn't valid"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ broken", "the user's file is as they left it");
    }
}
