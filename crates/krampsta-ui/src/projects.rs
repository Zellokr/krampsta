//! The project folder: which one the agent's tools work in, and opening another.
//!
//! The folder is chosen in the native dialog (or given at launch), opened off the UI thread by
//! the [`ProjectOpener`] the app wires in, and only then do runs get the new tools. A project
//! can't change under a running agent or unreviewed changes: those belong to the folder open now.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use krampsta_agent::{OpenedProject, ProjectOpened, ProjectOpener};
use rune::{FileDialogOptions, Tx};

use crate::state::KrampstaApp;

/// The project open now, as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectInfo {
    pub root: PathBuf,
    pub name: String,
}

impl ProjectInfo {
    fn of(project: &OpenedProject) -> Self {
        ProjectInfo { root: project.root.clone(), name: project.name() }
    }

    /// The folder as a person reads it (no `\\?\` prefix from resolving it on Windows).
    pub fn display_root(&self) -> String {
        display_path(&self.root)
    }
}

/// `path` without the verbatim prefix Windows puts on resolved paths.
pub fn display_path(path: &Path) -> String {
    let shown = path.display().to_string();
    shown.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(shown)
}

impl KrampstaApp {
    /// Starts with `project` open: runs get its tools.
    pub fn with_project(mut self, project: OpenedProject) -> Self {
        self.project = Some(ProjectInfo::of(&project));
        self.agent.set_toolbox(Some(project.tools));
        self
    }

    /// Shows the example project (stills): the same folder on every machine, with no tools.
    pub fn with_sample_project(mut self) -> Self {
        self.project = Some(crate::sample::project());
        self
    }

    /// Lets the user open other folders, which `opener` turns into tools.
    pub fn with_opener(mut self, opener: Arc<dyn ProjectOpener>) -> Self {
        self.opener = Some(opener);
        self
    }

    /// Why another folder can't be opened right now, if it can't.
    pub fn project_locked(&self) -> Option<&'static str> {
        if self.sessions.iter().any(|s| s.status.is_running()) {
            Some("Stop the agent before opening another folder.")
        } else if !self.review.proposals().is_empty() {
            Some("Apply or reject the proposed changes before opening another folder.")
        } else {
            None
        }
    }

    /// Shows the native folder dialog (a handler's job); the folder picked is then opened.
    pub fn choose_project(&mut self, tx: &mut Tx) {
        if let Some(why) = self.project_locked() {
            self.notice = Some(why.to_owned());
            return;
        }
        let options = FileDialogOptions {
            title: "Open a project folder".into(),
            directory: self.project.as_ref().map(|p| p.root.clone()),
            ..FileDialogOptions::default()
        };
        tx.open_folder(options, |s: &mut KrampstaApp, result, _tx: &mut Tx| match result {
            Ok(Some(path)) => s.open_project(path),
            Ok(None) => {}
            Err(e) => s.notice = Some(format!("The folder dialog couldn't open: {e}")),
        });
    }

    /// Opens `path` as the project in the background; it becomes the project in
    /// [`pump`](Self::pump) once the folder checks out.
    pub fn open_project(&mut self, path: PathBuf) {
        if let Some(why) = self.project_locked() {
            self.notice = Some(why.to_owned());
            return;
        }
        let Some(opener) = self.opener.clone() else {
            self.notice = Some("This build can't open folders.".into());
            return;
        };
        match self.agent.open_project(opener, path.clone()) {
            Ok(()) => {
                self.opening_project = Some(path);
                self.notice = None;
            }
            Err(e) => self.notice = Some(e.to_string()),
        }
    }

    /// Switches to the folders that finished opening (the last one asked for wins).
    pub(crate) fn land_projects(&mut self, opened: Vec<ProjectOpened>) -> bool {
        let mut changed = false;
        for ProjectOpened { path, result } in opened {
            if self.opening_project.as_ref() != Some(&path) {
                // Superseded by a later choice.
                continue;
            }
            self.opening_project = None;
            changed = true;
            match result {
                // Something started while it opened: the current folder stays.
                Ok(_) if self.project_locked().is_some() => {
                    self.notice = self.project_locked().map(str::to_owned);
                }
                Ok(project) => {
                    self.project = Some(ProjectInfo::of(&project));
                    self.agent.set_toolbox(Some(project.tools));
                    self.notice = None;
                }
                Err(why) => self.notice = Some(why),
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use krampsta_agent::{MockCatalog, MockConfig, Toolbox};
    use krampsta_core::FileChange;

    use super::*;

    /// No tools at all: what a project needs here is only its root.
    struct NoTools;

    impl Toolbox for NoTools {
        fn specs(&self) -> Arc<[krampsta_agent::ToolSpec]> {
            Arc::from(Vec::new())
        }
        fn run(&self, _: &str, _: &str, _: &krampsta_agent::Call<'_>) -> Result<krampsta_agent::ToolOutput, String> {
            Err("no tools".into())
        }
    }

    /// Opens any folder whose name doesn't start with "missing".
    struct Opens;

    impl ProjectOpener for Opens {
        fn open(&self, root: &Path) -> Result<OpenedProject, String> {
            if root.file_name().is_some_and(|n| n.to_string_lossy().starts_with("missing")) {
                return Err(format!("{} doesn't exist", root.display()));
            }
            Ok(OpenedProject { root: root.to_owned(), tools: Arc::new(NoTools) })
        }
    }

    fn app() -> KrampstaApp {
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())))
            .with_project(OpenedProject { root: PathBuf::from("/code/first"), tools: Arc::new(NoTools) })
            .with_opener(Arc::new(Opens))
    }

    fn pump_until_opened(app: &mut KrampstaApp) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.opening_project.is_some() {
            assert!(Instant::now() < deadline, "the folder never opened");
            app.pump();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn opening_a_folder_switches_the_project_once_it_checks_out() {
        let mut app = app();
        assert_eq!(app.project.as_ref().map(|p| p.name.as_str()), Some("first"));
        app.open_project(PathBuf::from("/code/second"));
        assert_eq!(app.opening_project, Some(PathBuf::from("/code/second")));
        assert_eq!(app.project.as_ref().map(|p| p.name.as_str()), Some("first"), "not until it opened");
        pump_until_opened(&mut app);
        assert_eq!(app.project.as_ref().map(|p| p.name.as_str()), Some("second"));
        assert_eq!(app.notice, None);
    }

    #[test]
    fn a_folder_that_cant_open_keeps_the_project_and_says_why() {
        let mut app = app();
        app.open_project(PathBuf::from("/code/missing"));
        pump_until_opened(&mut app);
        assert_eq!(app.project.as_ref().map(|p| p.name.as_str()), Some("first"));
        assert!(app.notice.as_deref().is_some_and(|n| n.ends_with("doesn't exist")), "{:?}", app.notice);
    }

    #[test]
    fn the_project_stays_while_changes_wait_for_review() {
        let mut app = app();
        app.stage_proposal(FileChange { path: "a.txt".into(), before: Some("old\n".into()), after: "new\n".into() });
        app.open_project(PathBuf::from("/code/second"));
        assert_eq!(app.opening_project, None, "nothing is opened");
        assert!(app.notice.as_deref().is_some_and(|n| n.starts_with("Apply or reject")));
    }

    #[test]
    fn the_folder_shows_without_the_windows_verbatim_prefix() {
        assert_eq!(display_path(Path::new(r"\\?\C:\code\krampsta")), r"C:\code\krampsta");
        assert_eq!(display_path(Path::new("/code/krampsta")), "/code/krampsta");
    }
}
