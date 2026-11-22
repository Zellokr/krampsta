//! The servers the user configured, in `mcp.json` (Krampsta's config folder), in the shape
//! Claude Desktop, Claude Code and Cursor use, so a file from any of them works as it is:
//!
//! ```json
//! {"mcpServers": {
//!   "github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"],
//!              "env": {"GITHUB_TOKEN": "…"}},
//!   "docs": {"type": "http", "url": "https://example.com/mcp",
//!            "headers": {"Authorization": "Bearer …"}},
//!   "old": {"command": "old-server", "disabled": true}
//! }}
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Transport;

/// One configured server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    /// Its key in `mcpServers`; part of its tools' names.
    pub name: String,
    pub transport: Transport,
    /// Kept in the file but not started.
    pub disabled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{path} can't be read: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("{path} isn't valid: {source}")]
    Invalid { path: PathBuf, source: serde_json::Error },
    #[error("the MCP server `{name}` {problem}")]
    Server { name: String, problem: String },
}

/// Where `mcp.json` lives: `<config folder>/krampsta/mcp.json`, beside the permission rules.
pub fn default_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("krampsta").join("mcp.json"))
}

#[derive(Deserialize)]
struct File {
    #[serde(default, rename = "mcpServers")]
    servers: BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    disabled: bool,
}

/// Reads the servers in `file`. A missing file is none configured. Blocking: read it at start,
/// off any async worker.
pub fn load(file: &Path) -> Result<Vec<ServerConfig>, ConfigError> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(ConfigError::Read { path: file.to_owned(), source }),
    };
    let parsed: File =
        serde_json::from_str(&text).map_err(|source| ConfigError::Invalid { path: file.to_owned(), source })?;
    parsed.servers.into_iter().map(|(name, entry)| server(name, entry)).collect()
}

/// A server's name: letters, digits, `-` and `_` (it goes into its tools' names).
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn server(name: String, entry: Entry) -> Result<ServerConfig, ConfigError> {
    let problem = |problem: &str| ConfigError::Server { name: name.clone(), problem: problem.to_owned() };
    if !valid_name(&name) {
        return Err(problem("needs a name of letters, digits, `-` and `_` only"));
    }
    let transport = match (entry.kind.as_deref(), entry.command, entry.url) {
        (None | Some("stdio"), Some(command), None) => {
            Transport::Stdio { command, args: entry.args, env: entry.env, cwd: entry.cwd }
        }
        (None | Some("http" | "streamable-http" | "streamableHttp"), None, Some(url)) => {
            Transport::Http { url, headers: entry.headers }
        }
        (Some("sse"), _, _) => {
            return Err(problem("uses the old SSE transport, which Krampsta doesn't speak: use its HTTP endpoint"));
        }
        (Some(kind @ ("stdio" | "http" | "streamable-http" | "streamableHttp")), _, _) => {
            return Err(problem(&format!("has type `{kind}` but not just its `command` or its `url`")));
        }
        (Some(kind), _, _) => return Err(problem(&format!("has an unknown type `{kind}`"))),
        (None, _, _) => return Err(problem("needs either a `command` or a `url`")),
    };
    Ok(ServerConfig { name, transport, disabled: entry.disabled })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<Vec<ServerConfig>, ConfigError> {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mcp.json");
        std::fs::write(&file, json).unwrap();
        load(&file)
    }

    #[test]
    fn a_claude_desktop_file_reads_as_it_is() {
        let servers = parse(
            r#"{"mcpServers": {
                "github": {"command": "npx", "args": ["-y", "server-github"], "env": {"GITHUB_TOKEN": "t"}},
                "docs": {"type": "http", "url": "https://example.com/mcp", "headers": {"X-Key": "k"}},
                "old": {"command": "old", "disabled": true}
            }}"#,
        )
        .unwrap();
        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["docs", "github", "old"]);
        assert!(
            matches!(&servers[0].transport, Transport::Http { url, headers } if url.ends_with("/mcp") && headers.len() == 1)
        );
        assert!(
            matches!(&servers[1].transport, Transport::Stdio { command, env, .. } if command == "npx" && env.len() == 1)
        );
        assert!(servers[2].disabled && !servers[1].disabled);
    }

    #[test]
    fn no_file_is_no_servers() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(&dir.path().join("mcp.json")).unwrap().is_empty());
        assert!(parse("{}").unwrap().is_empty());
    }

    #[test]
    fn mistakes_name_the_server_and_say_what_is_wrong() {
        let cases = [
            (r#"{"mcpServers": {"a": {}}}"#, "needs either a `command` or a `url`"),
            (r#"{"mcpServers": {"a": {"type": "sse", "url": "http://x"}}}"#, "old SSE transport"),
            (r#"{"mcpServers": {"a": {"type": "ws", "url": "ws://x"}}}"#, "unknown type `ws`"),
            (r#"{"mcpServers": {"a": {"command": "x", "url": "http://x"}}}"#, "needs either"),
            (r#"{"mcpServers": {"my server": {"command": "x"}}}"#, "letters, digits"),
        ];
        for (json, expected) in cases {
            let error = parse(json).unwrap_err().to_string();
            assert!(error.contains(expected), "{json}: {error}");
        }
        assert!(matches!(parse("{not json"), Err(ConfigError::Invalid { .. })));
    }
}
