//! Krune — Native AI Coding Agent.
//!
//! The executable only wires things together: it picks the agent engine (the `MockAgent` for
//! now) and hands it to the UI. Swapping in a real provider means changing the engine here,
//! not the interface.
//!
//! `cargo run -p krune-app` opens the window. Debug builds (or `--features tooling`) also run
//! headless: `-- --snap krune.png --size 1200x800 --scale 1`, `-- --tree krune-tree.json`,
//! `-- --list-scenes`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;

use krune_agent::{AgentEngine, MockAgent, MockConfig};
use krune_ui::KruneApp;

fn engine() -> Arc<dyn AgentEngine> {
    Arc::new(MockAgent::new(MockConfig::default()))
}

fn main() {
    rune::prewarm_gpu();
    rune::App::new(krune_ui::spec()).chrome(krune_ui::chrome()).init(|| KruneApp::new(engine())).run();
}
