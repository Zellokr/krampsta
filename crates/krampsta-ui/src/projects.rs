//! The project folder: which one the agent's tools work in, and opening another.
//!
//! The folder is chosen in the native dialog (or given at launch), opened off the UI thread by
//! the [`ProjectOpener`] the app wires in, and only then do runs get the new tools. A project
//! can't change under a running agent or unreviewed changes: those belong to the folder open now.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use krampsta_agent::{OpenedProject, ProjectOpened, ProjectOpener, project_name};
use krampsta_core::Session;
use rune::{FileDialogOptions, Tx};

use crate::state::{KrampstaApp, SendError};

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

/// The sessions the sidebar lists under one project's heading.
#[derive(Debug)]
pub struct Group<'a> {
    /// None: no project is open and these sessions aren't tied to one.
    pub root: Option<&'a Path>,
    pub name: String,
    /// Newest first.
    pub sessions: Vec<&'a Session>,
}

impl KrampstaApp {
    /// The sessions by project: the open one first, then the others by their latest session,
    /// and sessions with no project last. A session not tied to a folder yet counts as the
    /// open project's (it takes that one when it runs). Empty groups are left out.
    pub fn groups(&self) -> Vec<Group<'_>> {
        let open = self.project.as_ref().map(|p| p.root.as_path());
        let mut groups: Vec<Group<'_>> = Vec::new();
        if let Some(root) = open {
            groups.push(Group { root: Some(root), name: project_name(root), sessions: Vec::new() });
        }
        for session in &self.sessions {
            let root = session.project.as_deref().or(open);
            match groups.iter_mut().find(|g| g.root == root) {
                Some(group) => group.sessions.push(session),
                None => groups.push(Group {
                    root,
                    name: root.map_or_else(|| "No project".to_owned(), project_name),
                    sessions: vec![session],
                }),
            }
        }
        groups.retain(|g| !g.sessions.is_empty());
        // Stable: the order above stays within each side.
        groups.sort_by_key(|g| g.root.is_none());
        groups
    }

    pub fn is_group_collapsed(&self, root: Option<&Path>) -> bool {
        self.collapsed_groups.contains(&root.map(Path::to_path_buf))
    }

    /// Folds a project's sessions away, or shows them again.
    pub fn toggle_group(&mut self, root: Option<PathBuf>) {
        if !self.collapsed_groups.remove(&root) {
            self.collapsed_groups.insert(root);
        }
    }

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
                    // The selected session may belong here now, or no longer.
                    self.notice =
                        self.foreign_project(self.selected).map(|name| SendError::OtherProject(name).to_string());
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
            .with_sample_sessions()
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

    /// A session from an earlier launch in `root`.
    fn from(app: &mut KrampstaApp, root: &str, title: &str) -> krampsta_core::SessionId {
        let id = app.new_session();
        let s = app.sessions.iter_mut().find(|s| s.id == id).unwrap();
        s.project = Some(PathBuf::from(root));
        s.title = title.into();
        id
    }

    fn group_names(app: &KrampstaApp) -> Vec<(String, usize)> {
        app.groups().iter().map(|g| (g.name.clone(), g.sessions.len())).collect()
    }

    #[test]
    fn sessions_group_by_project_open_one_first_then_by_their_latest_session() {
        let mut app = app();
        // The two samples aren't tied to a folder: they count as the open project's.
        from(&mut app, "/code/rune", "Older rune work");
        from(&mut app, "/code/other", "Other work");
        from(&mut app, "/code/rune", "Newer rune work");
        app.new_session();
        assert_eq!(group_names(&app), [("first".into(), 3), ("rune".into(), 2), ("other".into(), 1)]);
        let rune = &app.groups()[1];
        assert_eq!(rune.sessions[0].title, "Newer rune work", "newest first inside a group");
    }

    #[test]
    fn with_no_project_open_untied_sessions_go_last_under_no_project() {
        let mut app = app();
        app.project = None;
        from(&mut app, "/code/rune", "Rune work");
        let names = group_names(&app);
        assert_eq!(names.last(), Some(&("No project".into(), 2)));
        assert_eq!(names[0], ("rune".into(), 1));
    }

    #[test]
    fn a_session_from_another_project_says_so_and_wont_run_here() {
        let mut app = app();
        let theirs = from(&mut app, "/code/rune", "Rune work");
        app.select(theirs);
        assert_eq!(app.notice.as_deref(), Some("This session is from rune. Open that folder to continue it."));
        app.set_draft("go on");
        assert!(matches!(app.send(), Err(SendError::OtherProject(name)) if name == "rune"));
        assert_eq!(app.session(theirs).map(|s| s.messages.len()), Some(0), "nothing was sent");

        // Once that folder is open, it goes on there.
        app.open_project(PathBuf::from("/code/rune"));
        pump_until_opened(&mut app);
        assert_eq!(app.notice, None, "the session belongs to the open project now");
        assert!(app.send().is_ok());
    }

    #[test]
    fn an_untied_session_takes_the_open_project_when_it_runs() {
        let mut app = app();
        let sample = app.sessions.iter().find(|s| s.project.is_none()).map(|s| s.id).unwrap();
        app.select(sample);
        app.set_draft("hello");
        app.send().unwrap();
        assert_eq!(app.session(sample).and_then(|s| s.project.clone()), Some(PathBuf::from("/code/first")));
    }

    #[test]
    fn groups_fold_and_a_new_session_unfolds_its_own() {
        let mut app = app();
        let first = Some(PathBuf::from("/code/first"));
        app.toggle_group(first.clone());
        assert!(app.is_group_collapsed(first.as_deref()));
        app.new_session();
        assert!(!app.is_group_collapsed(first.as_deref()));
    }

    #[test]
    fn the_folder_shows_without_the_windows_verbatim_prefix() {
        assert_eq!(display_path(Path::new(r"\\?\C:\code\krampsta")), r"C:\code\krampsta");
        assert_eq!(display_path(Path::new("/code/krampsta")), "/code/krampsta");
    }
}
