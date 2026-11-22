//! The project's files, as the right panel's "Files" view shows them: a tree (rune-kit's
//! `FileTree`), a filter, and opening a file to mention it in the composer. The list comes from
//! the tools (`Toolbox::files`) through the host, off the UI thread, and lands in `pump`; it is
//! asked for when the view first shows, when the project changes, when a change is written, and
//! by its refresh button. A list that arrives for a request that is no longer the last is dropped.

use krampsta_agent::FileList;
use rune::TextEdit;
use rune_kit::{FileNode, FileTree, TreeModel};

use crate::state::KrampstaApp;

/// What the right panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Inspector {
    /// The changes the agent proposed, to review.
    #[default]
    Changes,
    /// What differs from the repository's last commit (`git.rs`).
    Git,
    /// The project's files.
    Files,
}

/// Where the list of files is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Listing {
    /// Not asked for yet, or out of date: the view asks when it next shows.
    #[default]
    Stale,
    Loading,
    Ready,
    Failed(String),
}

/// The tree and what goes with it.
pub struct ProjectTree {
    pub model: TreeModel<FileNode<()>>,
    pub filter: TextEdit,
    pub listing: Listing,
    /// The list stopped at its cap.
    pub truncated: bool,
    /// The number of the last request; answers to older ones are dropped.
    ask: u64,
}

impl Default for ProjectTree {
    fn default() -> Self {
        ProjectTree {
            model: TreeModel::new(),
            filter: TextEdit::new(""),
            listing: Listing::Stale,
            truncated: false,
            ask: 0,
        }
    }
}

impl KrampstaApp {
    /// Shows `inspector` in the right panel, opening it, and reads what it shows if it isn't yet.
    pub fn show_inspector(&mut self, inspector: Inspector) {
        self.inspector = inspector;
        if !self.changes.is_open() {
            self.toggle_changes();
        }
        if inspector == Inspector::Git && self.repo.listing == Listing::Stale {
            self.refresh_repo();
        }
        if inspector == Inspector::Files && self.files.listing == Listing::Stale {
            self.refresh_files();
        }
    }

    /// Asks for the project's files again.
    pub fn refresh_files(&mut self) {
        self.files.ask += 1;
        self.files.listing = Listing::Loading;
        if let Err(e) = self.agent.list_files(self.files.ask) {
            self.files.listing = Listing::Failed(e.to_string());
        }
    }

    /// The folder changed (another project, a written change): the file list and the Git diff
    /// are read again if they show, else when they next do.
    pub(crate) fn disk_changed(&mut self) {
        self.repo_changed();
        if self.inspector == Inspector::Files && self.changes.is_open() {
            self.refresh_files();
        } else {
            // A request still in flight is out of date too.
            self.files.ask += 1;
            self.files.listing = Listing::Stale;
        }
    }

    /// Lands the lists that arrived. Only the answer to the last request counts. True if one did.
    pub(crate) fn land_files(&mut self, lists: Vec<FileList>) -> bool {
        let Some(list) = lists.into_iter().rfind(|l| l.ask == self.files.ask) else { return false };
        match list.result {
            Ok(files) => {
                // Rebuilt once per list, not per frame; what was open and selected stays so.
                let open = expanded(&self.files.model);
                let selected = self.files.model.cursor().map(str::to_owned);
                let mut model = FileTree::new().build(files.paths.into_iter().map(|p| (p, ())));
                for id in &open {
                    if model.contains(id) {
                        model.set_expanded(id, true);
                    }
                }
                if let Some(id) = selected.filter(|id| model.contains(id)) {
                    model.select(&id);
                }
                self.files.model = model;
                self.files.truncated = files.truncated;
                self.files.listing = Listing::Ready;
                self.apply_file_filter();
            }
            Err(why) => self.files.listing = Listing::Failed(why),
        }
        true
    }

    /// Keeps the folders that lead to a file whose path holds the filter's text.
    pub(crate) fn apply_file_filter(&mut self) {
        let query = self.files.filter.text.trim().to_lowercase();
        if query.is_empty() {
            self.files.model.clear_filter();
        } else {
            self.files.model.filter(|n| !n.dir && n.path.to_lowercase().contains(&query));
        }
    }

    /// Mentions the file `path` in the active session's composer, where the caret is.
    pub fn open_file(&mut self, path: &str) {
        if self.files.model.get(path).is_none_or(|n| n.dir) {
            return;
        }
        let session = self.selected;
        let draft = &mut self.view_mut(session).draft;
        let (start, _) = draft.range();
        let before = draft.text[..start].chars().next_back();
        if before.is_some_and(|c| !c.is_whitespace()) {
            draft.insert(" ");
        }
        draft.insert(path);
        draft.insert(" ");
        self.focus_composer(session);
    }
}

/// The `files` scene: the example project's files with `src/net` open, a file selected, and
/// the example changes proposed (their files marked).
pub(crate) fn scene(s: &mut KrampstaApp) {
    let paths = [
        ".gitignore",
        "Cargo.toml",
        "README.md",
        "config/retry.toml",
        "config/server.toml",
        "docs/architecture.md",
        "docs/retry.md",
        "src/main.rs",
        "src/lib.rs",
        "src/net/backoff.rs",
        "src/net/client.rs",
        "src/net/mod.rs",
        "src/storage/cache.rs",
        "src/storage/mod.rs",
        "tests/retry.rs",
    ];
    for change in crate::sample::changes() {
        s.propose(change);
    }
    s.files.ask += 1;
    let files =
        krampsta_agent::ProjectFiles { paths: paths.iter().map(|p| (*p).to_owned()).collect(), truncated: false };
    s.land_files(vec![FileList { ask: s.files.ask, result: Ok(files) }]);
    for open in ["src", "src/net", "config"] {
        s.files.model.set_expanded(open, true);
    }
    s.files.model.select("src/net/client.rs");
    s.inspector = Inspector::Files;
}

/// Every open folder in `model`.
fn expanded(model: &TreeModel<FileNode<()>>) -> Vec<String> {
    let mut open = Vec::new();
    let mut stack: Vec<&str> = model.children(None).collect();
    while let Some(id) = stack.pop() {
        if model.is_expanded(id) {
            open.push(id.to_owned());
            stack.extend(model.children(Some(id)));
        }
    }
    open
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::{MockCatalog, MockConfig, ProjectFiles};

    use super::*;

    fn app() -> KrampstaApp {
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())))
    }

    fn list(ask: u64, paths: &[&str]) -> FileList {
        let paths = paths.iter().map(|p| (*p).to_owned()).collect();
        FileList { ask, result: Ok(ProjectFiles { paths, truncated: false }) }
    }

    #[test]
    fn only_the_answer_to_the_last_request_lands_and_open_folders_stay_open() {
        let mut app = app();
        app.files.ask = 2;
        assert!(!app.land_files(vec![list(1, &["old.rs"])]), "an older request's answer is dropped");
        assert!(app.land_files(vec![list(2, &["src/main.rs", "src/lib.rs", "README.md"])]));
        assert_eq!(app.files.listing, Listing::Ready);
        assert!(app.files.model.contains("src/main.rs"));
        app.files.model.set_expanded("src", true);

        app.files.ask = 3;
        app.land_files(vec![list(3, &["src/main.rs", "src/new.rs"])]);
        assert!(app.files.model.is_expanded("src"), "a refresh keeps what was open");
        assert!(app.files.model.contains("src/new.rs"));
    }

    #[test]
    fn opening_a_file_mentions_it_where_the_caret_is() {
        let mut app = app();
        app.files.ask = 1;
        app.land_files(vec![list(1, &["src/main.rs", "src/lib.rs"])]);
        app.set_draft("Look at");
        app.open_file("src/main.rs");
        app.open_file("src");
        assert_eq!(app.draft().text, "Look at src/main.rs ", "a folder isn't mentioned");
        app.open_file("src/lib.rs");
        assert_eq!(app.draft().text, "Look at src/main.rs src/lib.rs ");
    }

    #[test]
    fn the_filter_keeps_matching_files_and_a_stale_list_is_asked_for_when_shown() {
        let mut app = app();
        app.files.ask = 1;
        app.land_files(vec![list(1, &["src/main.rs", "docs/guide.md"])]);
        app.files.filter = TextEdit::new("GUIDE");
        app.apply_file_filter();
        assert!(app.files.model.is_match("docs/guide.md"));
        assert!(!app.files.model.is_match("src/main.rs"));

        app.disk_changed();
        assert_eq!(app.files.listing, Listing::Stale, "hidden: listed when it next shows");
        app.show_inspector(Inspector::Files);
        assert_eq!(app.files.listing, Listing::Loading);
        assert_eq!(app.inspector, Inspector::Files);
    }
}
