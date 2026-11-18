//! Krune — Native AI Coding Agent.
//!
//! The executable only wires things together: it picks the agent engine and hands it to the
//! UI. A provider configured in the environment (see `krune_providers::engine_from_env`) wins;
//! otherwise Krune runs the simulated `MockAgent`.
//!
//! `cargo run -p krune-app` opens the window. Debug builds (or `--features tooling`) also run
//! headless: `-- --snap krune.png --size 1200x800 --scale 1`, `-- --tree krune-tree.json`,
//! `-- --list-scenes`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;

use krune_agent::{AgentEngine, MockAgent, MockConfig};
use krune_ui::KruneApp;

fn engine() -> Arc<dyn AgentEngine> {
    match krune_providers::engine_from_env() {
        Ok(Some(engine)) => engine,
        Ok(None) => Arc::new(MockAgent::new(MockConfig::default())),
        Err(e) => {
            // The composer shows "engine: mock", so the fallback is visible in the window too.
            eprintln!("krune: {e}; using the simulated agent instead");
            Arc::new(MockAgent::new(MockConfig::default()))
        }
    }
}

fn main() {
    rune::prewarm_gpu();
    rune::App::new(krune_ui::spec()).chrome(krune_ui::chrome()).init(|| KruneApp::new(engine())).run();
}
