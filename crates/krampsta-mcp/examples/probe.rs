//! Starts the servers in `mcp.json` as Krampsta does and says where each one got: its tools, or
//! why it failed. A quick check of a configuration without opening the window.
//!
//! ```text
//! cargo run -p krampsta-mcp --example probe                 # the user's mcp.json
//! cargo run -p krampsta-mcp --example probe -- path/to/mcp.json
//! ```

use std::time::{Duration, Instant};

use krampsta_agent::{ServerState, ToolServers};
use krampsta_mcp::{McpServers, config};

fn main() {
    let file = std::env::args().nth(1).map(Into::into).or_else(config::default_file);
    let Some(file) = file else {
        eprintln!("probe: no config folder on this system; pass the file");
        std::process::exit(2);
    };
    let configs = match config::load(&file) {
        Ok(configs) => configs,
        Err(e) => {
            eprintln!("probe: {e}");
            std::process::exit(1);
        }
    };
    println!("{}: {} server(s)", file.display(), configs.len());
    let servers = match McpServers::start(configs, None) {
        Ok(servers) => servers,
        Err(e) => {
            eprintln!("probe: {e}");
            std::process::exit(1);
        }
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while servers.statuses().iter().any(|s| s.state == ServerState::Connecting) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    for server in servers.statuses() {
        match server.state {
            ServerState::Ready { tools } => {
                println!("  {}: connected, {} tool(s): {}", server.name, tools.len(), tools.join(", "))
            }
            ServerState::Failed(why) => println!("  {}: failed: {why}", server.name),
            ServerState::Connecting => println!("  {}: still connecting after a minute", server.name),
            ServerState::Disabled => println!("  {}: disabled", server.name),
        }
    }
}
