//! Krampsta — Native AI Coding Agent.
//!
//! The executable only wires things together: it builds the provider catalog from the
//! environment (see `krampsta_providers::Catalog`) and hands it to the UI, where the user picks
//! the provider and model. If the environment is invalid, Krampsta runs the simulated agent alone.
//!
//! `cargo run -p krampsta-app [-- <folder>]` opens the window, with `<folder>` (or the current
//! one) as the project. Debug builds (or `--features tooling`) also run
//! headless: `-- --snap krampsta.png --size 1200x800 --scale 1`, `-- --tree krampsta-tree.json`,
//! `-- --list-scenes`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use krampsta_agent::{EngineCatalog, MockCatalog, MockConfig, OpenedProject, SessionStore};
use krampsta_tools::WorkspaceOpener;
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

/// The folder `krampsta <folder>` names, or the one Krampsta was started in. Err: why there is
/// no project (the agent then runs without tools until the user opens a folder).
fn project() -> Result<OpenedProject, String> {
    let named = rune::extra_args().iter().find(|a| !a.starts_with('-'));
    let folder = match named {
        Some(folder) => PathBuf::from(folder),
        None => std::env::current_dir().map_err(|e| format!("the current folder can't be read: {e}"))?,
    };
    krampsta_tools::Workspace::open_project(&folder)
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
            let mut app = KrampstaApp::new(catalog()).with_opener(Arc::new(WorkspaceOpener));
            if rune::is_headless() {
                // Stills show the same folder on every machine; the simulated agent needs no tools.
                app = app.with_sample_project();
            } else {
                match project() {
                    Ok(project) => app = app.with_project(project),
                    Err(why) => {
                        eprintln!("krampsta: {why}; the agent runs without tools until a folder is opened");
                        app.notice = Some(why);
                    }
                }
            }
            if let Some(store) = store() {
                app = app.with_store(store);
            }
            app
        })
        .run();
}
