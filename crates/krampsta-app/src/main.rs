//! Krampsta — Native AI Coding Agent.
//!
//! The executable only wires things together: it builds the provider catalog from the
//! environment (see `krampsta_providers::Catalog`) and hands it to the UI, where the user picks
//! the provider and model. If the environment is invalid, Krampsta runs the simulated agent alone.
//!
//! `cargo run -p krampsta-app` opens the window. Debug builds (or `--features tooling`) also run
//! headless: `-- --snap krampsta.png --size 1200x800 --scale 1`, `-- --tree krampsta-tree.json`,
//! `-- --list-scenes`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;

use krampsta_agent::{EngineCatalog, MockCatalog, MockConfig};
use krampsta_ui::KrampstaApp;

fn catalog() -> Arc<dyn EngineCatalog> {
    match krampsta_providers::Catalog::from_env() {
        Ok(catalog) => Arc::new(catalog),
        Err(e) => {
            // The pickers then offer only "Simulated", so the fallback is visible in the window.
            eprintln!("krampsta: {e}; using the simulated agent instead");
            Arc::new(MockCatalog::new(MockConfig::default()))
        }
    }
}

fn main() {
    rune::prewarm_gpu();
    rune::App::new(krampsta_ui::spec()).chrome(krampsta_ui::chrome()).init(|| KrampstaApp::new(catalog())).run();
}
