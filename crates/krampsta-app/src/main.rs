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

use krampsta_agent::{EngineCatalog, MockCatalog, MockConfig, SessionStore, Toolbox};
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

/// The tools, over the folder Krampsta was started in (choosing another comes with projects).
fn tools() -> Option<Arc<dyn Toolbox>> {
    match std::env::current_dir().and_then(krampsta_tools::Workspace::open) {
        Ok(workspace) => Some(Arc::new(workspace)),
        Err(e) => {
            eprintln!("krampsta: the current folder can't be used as the project ({e}); the agent runs without tools");
            None
        }
    }
}

/// Where sessions are saved. None in headless runs (stills, scripts): they must neither depend
/// on nor add to this machine's history.
fn store() -> Option<Arc<dyn SessionStore>> {
    if rune::is_headless() {
        return None;
    }
    let Some(dir) = krampsta_store::default_dir() else {
        eprintln!("krampsta: no data folder on this system; sessions won't be saved");
        return None;
    };
    match krampsta_store::FileStore::open(dir) {
        Ok(store) => Some(Arc::new(store)),
        Err(e) => {
            eprintln!("krampsta: {e}; sessions won't be saved");
            None
        }
    }
}

fn main() {
    rune::prewarm_gpu();
    rune::App::new(krampsta_ui::spec())
        .chrome(krampsta_ui::chrome())
        .init(|| {
            let mut app = KrampstaApp::new(catalog());
            if let Some(tools) = tools() {
                app = app.with_tools(tools);
            }
            if let Some(store) = store() {
                app = app.with_store(store);
            }
            app
        })
        .run();
}
