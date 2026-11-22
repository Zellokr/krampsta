//! A fake MCP server over stdio, for `krampsta-mcp`'s tests only: one JSON-RPC message per
//! line in, one answer per line out ([`krampsta_mcp::fake::reply`]).
//!
//! `--crash` writes a line to stderr and exits before answering anything, as a server that is
//! missing its configuration would. A call to the `slow` tool is never answered.

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
        if message["params"]["name"] == "slow" {
            continue;
        }
        if let Some(answer) = krampsta_mcp::fake::reply(&message) {
            let mut out = stdout.lock();
            if writeln!(out, "{answer}").and_then(|()| out.flush()).is_err() {
                return;
            }
        }
    }
}
