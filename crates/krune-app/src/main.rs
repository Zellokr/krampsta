//! Krune — Native AI Coding Agent.
//!
//! The executable only wires things together: it builds the provider catalog from the
//! environment (see `krune_providers::Catalog`) and hands it to the UI, where the user picks
//! the provider and model. If the environment is invalid, Krune runs the simulated agent alone.
//!
//! `cargo run -p krune-app` opens the window. Debug builds (or `--features tooling`) also run
//! headless: `-- --snap krune.png --size 1200x800 --scale 1`, `-- --tree krune-tree.json`,
//! `-- --list-scenes`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;

use krune_agent::{EngineCatalog, MockCatalog, MockConfig};
use krune_ui::KruneApp;

fn catalog() -> Arc<dyn EngineCatalog> {
    match krune_providers::Catalog::from_env() {
        Ok(catalog) => Arc::new(catalog),
        Err(e) => {
            // The pickers then offer only "Simulated", so the fallback is visible in the window.
            eprintln!("krune: {e}; using the simulated agent instead");
            Arc::new(MockCatalog::new(MockConfig::default()))
        }
    }
}

fn main() {
    rune::prewarm_gpu();
    rune::App::new(krune_ui::spec()).chrome(krune_ui::chrome()).init(|| KruneApp::new(catalog())).run();
}
