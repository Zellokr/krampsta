//! Krampsta's client for MCP (Model Context Protocol) servers, over the official Rust SDK
//! (`rmcp`): [`McpClient::connect`] starts a server as a process and talks to it over its
//! stdin and stdout, or reaches one over Streamable HTTP; then it lists the server's tools
//! ([`McpClient::tools`]) and calls them ([`McpClient::call`]).
//!
//! A stdio server runs as a process tree that is killed whole when the client goes (a job
//! object on Windows, a process group on Unix), with no console window. What it writes to
//! stderr is kept (the last few KB) to say why it failed to start. Every call is cancellable
//! and its reply is capped, so a server can neither stall a run nor flood the model's context.
//!
//! [`config`] reads the servers the user set up (`mcp.json`); [`McpServers`] keeps them
//! connected while Krampsta runs, and [`ToolSet`] puts their tools in the agent's registry
//! beside the project's own. This crate knows nothing about the UI.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, JsonObject};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

pub mod config;
mod servers;

pub use config::{ConfigError, ServerConfig};
pub use servers::{McpServers, PREFIX, ToolSet, tool_name};

#[doc(hidden)]
pub mod fake;

/// How long a server has to answer `initialize`.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// The most of a reply that reaches the model, in bytes.
pub const MAX_REPLY: usize = 64 * 1024;
/// The most of a server's stderr that is kept, in bytes.
const STDERR_TAIL: usize = 4 * 1024;

/// How to reach a server, as the user configures it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Transport {
    /// A program started by Krampsta, spoken to over its stdin and stdout.
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
    },
    /// A server at a URL (Streamable HTTP). An `Authorization: Bearer …` header is sent as such.
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
    },
}

/// Environment values and headers often hold tokens: Debug shows only their names.
impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::Stdio { command, args, env, cwd } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", args)
                .field("env", &env.keys().collect::<Vec<_>>())
                .field("cwd", cwd)
                .finish(),
            Transport::Http { url, headers } => {
                f.debug_struct("Http").field("url", url).field("headers", &headers.keys().collect::<Vec<_>>()).finish()
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("couldn't start `{command}`: {source}")]
    Start { command: String, source: std::io::Error },
    #[error("the header `{0}` isn't valid")]
    Header(String),
    #[error("the MCP server didn't answer within {} seconds", CONNECT_TIMEOUT.as_secs())]
    Timeout,
    /// The handshake failed; `output` is the end of what the server wrote to stderr, if any.
    #[error("the MCP server couldn't be reached: {reason}{}", said(output))]
    Connect { reason: String, output: Option<String> },
    #[error("the MCP server answered with an error: {0}")]
    Service(String),
    #[error("the arguments must be a JSON object: {0}")]
    Arguments(String),
    #[error("stopped")]
    Cancelled,
}

fn said(output: &Option<String>) -> String {
    output.as_deref().map(|o| format!(". It said: {o}")).unwrap_or_default()
}

/// A tool a server offers, as it describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    /// JSON Schema of its arguments.
    pub input_schema: Value,
}

/// What a call answered: its content as text (capped at [`MAX_REPLY`]), and whether the tool
/// said it failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolReply {
    pub text: String,
    pub is_error: bool,
}

/// What a stdio server has written to stderr lately.
type Tail = Arc<Mutex<VecDeque<u8>>>;

/// A connection to one server. Dropping it closes the connection and stops a stdio server.
pub struct McpClient {
    service: RunningService<RoleClient, ClientConfig>,
    /// The name the server gave itself in the handshake.
    server: Option<String>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient").field("server", &self.server_name()).finish()
    }
}

impl McpClient {
    /// Starts or reaches the server and completes the MCP handshake.
    pub async fn connect(transport: &Transport) -> Result<Self, McpError> {
        match transport {
            Transport::Stdio { command, args, env, cwd } => {
                let (child, stderr) = spawn(command, args, env, cwd.as_ref())?;
                let tail = Tail::default();
                let reader = stderr.map(|stderr| tokio::spawn(keep_tail(stderr, tail.clone())));
                let connected = handshake(child).await;
                if connected.is_err()
                    && let Some(reader) = reader
                {
                    // A server that exits says why on its way out: let that last line land.
                    let _ = tokio::time::timeout(Duration::from_millis(500), reader).await;
                }
                connected.map_err(|e| with_output(e, &tail))
            }
            Transport::Http { url, headers } => {
                let mut config = StreamableHttpClientTransportConfig::with_uri(url.as_str());
                for (name, value) in headers {
                    if name.eq_ignore_ascii_case("authorization")
                        && let Some(token) = value.strip_prefix("Bearer ")
                    {
                        config.auth_header = Some(token.to_owned());
                        continue;
                    }
                    let header =
                        http::HeaderName::try_from(name.as_str()).map_err(|_| McpError::Header(name.clone()))?;
                    let value =
                        http::HeaderValue::try_from(value.as_str()).map_err(|_| McpError::Header(name.clone()))?;
                    config.custom_headers.insert(header, value);
                }
                handshake(StreamableHttpClientTransport::from_config(config)).await
            }
        }
    }

    /// The name the server gave itself.
    pub fn server_name(&self) -> Option<&str> {
        self.server.as_deref()
    }

    /// Every tool the server offers (all pages).
    pub async fn tools(&self) -> Result<Vec<McpTool>, McpError> {
        let tools = self.service.list_all_tools().await.map_err(|e| McpError::Service(e.to_string()))?;
        Ok(tools
            .into_iter()
            .map(|t| McpTool {
                name: t.name.into_owned(),
                description: t.description.map(|d| d.into_owned()).unwrap_or_default(),
                input_schema: Value::Object((*t.input_schema).clone()),
            })
            .collect())
    }

    /// Calls `tool` with `arguments` (a JSON object, as the model writes it; blank for none).
    /// Stops waiting when `cancel` fires.
    pub async fn call(&self, tool: &str, arguments: &str, cancel: &CancellationToken) -> Result<ToolReply, McpError> {
        let mut params = CallToolRequestParams::new(tool.to_owned());
        if !arguments.trim().is_empty() {
            let object: JsonObject = serde_json::from_str(arguments).map_err(|e| McpError::Arguments(e.to_string()))?;
            params.arguments = Some(object);
        }
        let result = tokio::select! {
            result = self.service.call_tool(params) => result.map_err(|e| McpError::Service(e.to_string()))?,
            () = cancel.cancelled() => return Err(McpError::Cancelled),
        };
        let mut text = String::new();
        for block in &result.content {
            let line = describe(&serde_json::to_value(block).unwrap_or_default());
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&line);
        }
        if text.is_empty()
            && let Some(structured) = &result.structured_content
        {
            text = structured.to_string();
        }
        Ok(ToolReply { text: cap(text), is_error: result.is_error.unwrap_or(false) })
    }

    /// Closes the connection (and stops a stdio server) now, rather than on drop.
    pub async fn close(self) -> Result<(), McpError> {
        self.service.cancel().await.map(|_| ()).map_err(|e| McpError::Service(e.to_string()))
    }
}

/// The client as servers see it.
fn client_config() -> ClientConfig {
    ClientConfig::new(ClientCapabilities::default(), Implementation::new("krampsta", env!("CARGO_PKG_VERSION")))
}

async fn handshake<T, E, A>(transport: T) -> Result<McpClient, McpError>
where
    T: rmcp::transport::IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    match tokio::time::timeout(CONNECT_TIMEOUT, client_config().serve(transport)).await {
        Ok(Ok(service)) => {
            let server = service.peer_info().and_then(|info| info.server_info.clone()).map(|s| s.name);
            Ok(McpClient { service, server })
        }
        Ok(Err(e)) => Err(McpError::Connect { reason: e.to_string(), output: None }),
        Err(_) => Err(McpError::Timeout),
    }
}

/// A connection error with the end of the server's stderr, if it wrote any.
fn with_output(error: McpError, tail: &Tail) -> McpError {
    let McpError::Connect { reason, .. } = error else { return error };
    let bytes: Vec<u8> = tail.lock().unwrap_or_else(PoisonError::into_inner).iter().copied().collect();
    let output = String::from_utf8_lossy(&bytes).trim().to_owned();
    McpError::Connect { reason, output: (!output.is_empty()).then_some(output) }
}

fn spawn(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&PathBuf>,
) -> Result<(TokioChildProcess, Option<tokio::process::ChildStderr>), McpError> {
    let start = |source| McpError::Start { command: command.to_owned(), source };
    // Found on PATH with its extension (`npx` is `npx.cmd` on Windows).
    let mut cmd = rmcp::transport::which_command(command).map_err(start)?;
    // Only what a program needs to run, never the rest of Krampsta's environment (API keys
    // live there): the server gets the configured variables on top.
    cmd.args(args).env_clear().envs(inherited_env()).envs(env);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let mut wrapped = process_wrap::tokio::CommandWrap::from(cmd);
    #[cfg(windows)]
    {
        use process_wrap::tokio::{CreationFlags, JobObject};
        use windows::Win32::System::Threading::CREATE_NO_WINDOW;
        wrapped.wrap(CreationFlags(CREATE_NO_WINDOW)).wrap(JobObject);
    }
    #[cfg(unix)]
    wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
    wrapped.wrap(process_wrap::tokio::KillOnDrop);
    TokioChildProcess::builder(wrapped).stderr(Stdio::piped()).spawn().map_err(start)
}

/// The variables a stdio server inherits, as the official TypeScript SDK chooses them
/// (`getDefaultEnvironment`): enough to find programs and the user's folders.
const INHERITED_ENV: &[&str] = if cfg!(windows) {
    &[
        "APPDATA",
        "HOMEDRIVE",
        "HOMEPATH",
        "LOCALAPPDATA",
        "PATH",
        "PATHEXT",
        "PROCESSOR_ARCHITECTURE",
        "PROGRAMFILES",
        "SYSTEMDRIVE",
        "SYSTEMROOT",
        "TEMP",
        "TMP",
        "USERNAME",
        "USERPROFILE",
        "WINDIR",
        "COMSPEC",
    ]
} else {
    &["HOME", "LOGNAME", "PATH", "SHELL", "TERM", "USER", "LANG", "TMPDIR"]
};

fn inherited_env() -> impl Iterator<Item = (&'static str, std::ffi::OsString)> {
    INHERITED_ENV.iter().filter_map(|name| std::env::var_os(name).map(|value| (*name, value)))
}

/// Keeps the last [`STDERR_TAIL`] bytes the server writes to stderr.
async fn keep_tail(mut stderr: tokio::process::ChildStderr, tail: Tail) {
    let mut buffer = [0u8; 1024];
    while let Ok(read) = stderr.read(&mut buffer).await {
        if read == 0 {
            return;
        }
        let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
        tail.extend(&buffer[..read]);
        let extra = tail.len().saturating_sub(STDERR_TAIL);
        tail.drain(..extra);
    }
}

/// One content block as text: text as it is, the rest named by kind.
fn describe(block: &Value) -> String {
    let field = |name: &str| block[name].as_str().unwrap_or_default();
    match field("type") {
        "text" => field("text").to_owned(),
        kind @ ("image" | "audio") => format!("[{kind}: {}]", field("mimeType")),
        "resource" => match block["resource"]["text"].as_str() {
            Some(text) => text.to_owned(),
            None => format!("[resource: {}]", block["resource"]["uri"].as_str().unwrap_or_default()),
        },
        "resource_link" => format!("[link: {}]", field("uri")),
        other => format!("[{other}]"),
    }
}

/// `text` cut to [`MAX_REPLY`] bytes at a character boundary, saying how much was left out.
fn cap(mut text: String) -> String {
    if text.len() <= MAX_REPLY {
        return text;
    }
    let mut end = MAX_REPLY;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text.len() - end;
    text.truncate(end);
    text.push_str(&format!("\n… ({cut} more bytes left out)"));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_reply_is_cut_at_a_character_boundary() {
        let text = "ñ".repeat(MAX_REPLY);
        let capped = cap(text);
        assert!(capped.len() < MAX_REPLY + 64);
        assert!(capped.ends_with("bytes left out)"), "{}", &capped[capped.len() - 40..]);
        assert_eq!(cap("short".into()), "short");
    }

    #[test]
    fn debug_never_shows_env_values_or_headers() {
        let stdio = Transport::Stdio {
            command: "server".into(),
            args: vec![],
            env: BTreeMap::from([("API_TOKEN".into(), "sk-secret".into())]),
            cwd: None,
        };
        let http = Transport::Http {
            url: "https://example.com/mcp".into(),
            headers: BTreeMap::from([("Authorization".into(), "Bearer sk-secret".into())]),
        };
        for t in [stdio, http] {
            let shown = format!("{t:?}");
            assert!(!shown.contains("sk-secret") && (shown.contains("API_TOKEN") || shown.contains("Authorization")));
        }
    }

    #[test]
    fn the_config_reads_as_users_write_it() {
        let stdio: Transport =
            serde_json::from_str(r#"{"type": "stdio", "command": "npx", "args": ["-y", "server"]}"#).unwrap();
        assert!(matches!(stdio, Transport::Stdio { ref command, ref args, .. } if command == "npx" && args.len() == 2));
        let http: Transport = serde_json::from_str(r#"{"type": "http", "url": "http://localhost:3000/mcp"}"#).unwrap();
        assert!(matches!(http, Transport::Http { ref url, .. } if url.ends_with("/mcp")));
    }
}
