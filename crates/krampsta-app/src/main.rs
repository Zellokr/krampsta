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

use std::path::Path;
use std::sync::Arc;

use krampsta_agent::{EngineCatalog, MockCatalog, MockConfig, OpenedProject, ProjectOpener, SessionStore};
use krampsta_mcp::{McpServers, ToolSet};
use krampsta_tools::WorkspaceOpener;
use krampsta_ui::KrampstaApp;
use krampsta_ui::prefs::Prefs;

/// Opens a folder as the project, its tools joined by the MCP servers'.
struct Opener {
    mcp: Arc<McpServers>,
}

impl Opener {
    fn wrap(&self, project: OpenedProject) -> OpenedProject {
        let tools = Arc::new(ToolSet::new(Some(project.tools), self.mcp.clone()));
        OpenedProject { root: project.root, tools }
    }
}

impl ProjectOpener for Opener {
    fn open(&self, root: &Path) -> Result<OpenedProject, String> {
        WorkspaceOpener.open(root).map(|project| self.wrap(project))
    }
}

/// The MCP servers in `mcp.json`, connecting in the background, and why they couldn't be, if
/// so (Krampsta then runs without them and says so).
fn mcp() -> (Option<Arc<McpServers>>, Option<String>) {
    let (configs, problem) = match krampsta_mcp::config::default_file().map(|f| krampsta_mcp::config::load(&f)) {
        Some(Ok(configs)) => (configs, None),
        Some(Err(e)) => (Vec::new(), Some(e.to_string())),
        None => (Vec::new(), None),
    };
    // Their tools join the runs that start after they are ready: nothing to redraw yet.
    match McpServers::start(configs, Arc::new(|| {})) {
        Ok(servers) => (Some(servers), problem),
        Err(e) => (None, Some(format!("the MCP servers can't start: {e}"))),
    }
}

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

/// The project at start: the folder `krampsta <folder>` names, else the one open when Krampsta
/// last closed (if it still opens), else the one it was started in. Err: why there is none
/// (the agent then runs without tools until the user opens a folder).
fn project(last: Option<&Path>) -> Result<OpenedProject, String> {
    if let Some(named) = rune::extra_args().iter().find(|a| !a.starts_with('-')) {
        return krampsta_tools::Workspace::open_project(Path::new(named));
    }
    if let Some(last) = last {
        match krampsta_tools::Workspace::open_project(last) {
            Ok(project) => return Ok(project),
            Err(why) => eprintln!("krampsta: the last project can't be opened again ({why})"),
        }
    }
    let here = std::env::current_dir().map_err(|e| format!("the current folder can't be read: {e}"))?;
    krampsta_tools::Workspace::open_project(&here)
}

/// The saved settings, read before the first frame so the window opens as it was left. Err:
/// why they couldn't be read (the app then starts from the defaults and says so).
fn prefs(file: Option<&Path>) -> Result<Prefs, String> {
    let Some(file) = file else { return Ok(Prefs::default()) };
    match krampsta_store::load_settings(file) {
        Ok(Some(value)) => Prefs::from_json(value),
        Ok(None) => Ok(Prefs::default()),
        Err(e) => Err(e.to_string()),
    }
}

/// Where sessions and settings are saved.
fn store(settings: Option<&Path>) -> Option<Arc<dyn SessionStore>> {
    let Some(dir) = krampsta_store::default_dir() else {
        eprintln!("krampsta: no data folder on this system; sessions won't be saved");
        return None;
    };
    match krampsta_store::FileStore::open(dir) {
        Ok(store) => {
            let store = match settings {
                Some(file) => store.with_settings_file(file),
                None => store,
            };
            Some(Arc::new(store))
        }
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
            if rune::is_headless() {
                // Stills show the same folder and defaults on every machine, and neither read nor
                // add to this one's history, settings or MCP servers; the simulated agent needs no
                // tools.
                return app.with_opener(Arc::new(WorkspaceOpener)).with_sample_project().with_sample_sessions();
            }
            let (mcp, mcp_problem) = mcp();
            let opener = mcp.map(|mcp| Arc::new(Opener { mcp }));
            app = match &opener {
                Some(opener) => app.with_opener(opener.clone()),
                None => app.with_opener(Arc::new(WorkspaceOpener)),
            };
            let settings = krampsta_store::default_settings_file();
            let (prefs, problem) = match prefs(settings.as_deref()) {
                Ok(prefs) => (prefs, None),
                Err(why) => (Prefs::default(), Some(why)),
            };
            match (project(prefs.project.as_deref()), &opener) {
                (Ok(project), Some(opener)) => app = app.with_project(opener.wrap(project)),
                (Ok(project), None) => app = app.with_project(project),
                (Err(why), opener) => {
                    eprintln!("krampsta: {why}; the agent runs without the project's tools until a folder is opened");
                    if let Some(opener) = opener {
                        // The MCP servers' tools work without a folder.
                        app = app.with_tools(Arc::new(ToolSet::new(None, opener.mcp.clone())));
                    }
                    app.notice = Some(why);
                }
            }
            if let Some(why) = mcp_problem {
                eprintln!("krampsta: {why}");
                app.notice = Some(why);
            }
            if let Some(store) = store(settings.as_deref()) {
                app = app.with_store(store);
            }
            // `KRAMPSTA_PROVIDER` picks the provider over the one used last.
            let engine = std::env::var_os("KRAMPSTA_PROVIDER").is_none();
            app = app.with_prefs(prefs, engine);
            if let Some(why) = problem {
                eprintln!("krampsta: {why}");
                app.notice = Some(why);
            }
            app
        })
        .run();
}
