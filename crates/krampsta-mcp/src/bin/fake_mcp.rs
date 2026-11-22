//! A fake MCP server over stdio, for `krampsta-mcp`'s tests only: one JSON-RPC message per
//! line in, one answer per line out ([`krampsta_mcp::fake::reply`]).
//!
//! `--crash` writes a line to stderr and exits before answering anything, as a server that is
//! missing its configuration would. A call to the `slow` tool is never answered; one to `env`
//! answers with the names of the environment variables it got.

use std::io::{BufRead, Write};

fn main() {
    if std::env::args().any(|a| a == "--crash") {
        eprintln!("fake-mcp: missing FAKE_TOKEN, can't start");
        std::process::exit(1);
    }
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { return };
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        // The names of the variables it was started with: what a server can see.
        let answer = if message["params"]["name"] == "env" {
            let mut names: Vec<String> = std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
            names.sort();
            let content = serde_json::json!([{"type": "text", "text": names.join("\n")}]);
            Some(serde_json::json!({"jsonrpc": "2.0", "id": message["id"], "result": {"content": content}}))
        } else {
            krampsta_mcp::fake::reply(&message)
        };
        if message["params"]["name"] == "slow" {
            continue;
        }
        if let Some(answer) = answer {
            let mut out = stdout.lock();
            if writeln!(out, "{answer}").and_then(|()| out.flush()).is_err() {
                return;
            }
        }
    }
}
