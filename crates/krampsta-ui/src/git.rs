//! What differs from the repository's last commit, as the right panel's "Git" view shows it:
//! a read-only diff of every changed file (rune-kit's `DiffView`). The diff comes from the
//! tools (`Toolbox::repo_diff`, which runs `git`) through the host, off the UI thread, and lands
//! in `pump`, where each file's lines are compared once (the tools cap how much text that is).
//! It is asked for when the view first shows and goes stale with the file list (`disk_changed`).

use krampsta_agent::RepoDiffList;
use rune_kit::diff::{DiffState, FileDiff};

use crate::files::{Inspector, Listing};
use crate::state::KrampstaApp;

/// The diff and what goes with it.
pub struct RepoView {
    pub diff: DiffState,
    pub listing: Listing,
    /// Changed files named but not compared (binary, or too big).
    pub skipped: Vec<String>,
    /// More files changed than were read.
    pub truncated: bool,
    /// The number of the last request; answers to older ones are dropped.
    ask: u64,
}

impl Default for RepoView {
    fn default() -> Self {
        let mut diff = DiffState::new(Vec::new());
        // Long lines wrap in the panel's narrow column, as in "Changes".
        diff.set_wrap(true);
        RepoView { diff, listing: Listing::Stale, skipped: Vec::new(), truncated: false, ask: 0 }
    }
}

impl KrampstaApp {
    /// Reads what differs from the last commit again.
    pub fn refresh_repo(&mut self) {
        self.repo.ask += 1;
        self.repo.listing = Listing::Loading;
        if let Err(e) = self.agent.diff_repo(self.repo.ask) {
            self.repo.listing = Listing::Failed(e.to_string());
        }
    }

    /// The folder changed: read the diff again if it shows, else when it next does.
    pub(crate) fn repo_changed(&mut self) {
        if self.inspector == Inspector::Git && self.changes.is_open() {
            self.refresh_repo();
        } else {
            self.repo.ask += 1;
            self.repo.listing = Listing::Stale;
        }
    }

    /// Lands the diffs that arrived; only the answer to the last request counts. True if one did.
    pub(crate) fn land_repo(&mut self, diffs: Vec<RepoDiffList>) -> bool {
        let Some(answer) = diffs.into_iter().rfind(|d| d.ask == self.repo.ask) else { return false };
        match answer.result {
            Ok(repo) => {
                let files: Vec<FileDiff> = repo
                    .changes
                    .iter()
                    .map(|c| {
                        let old_path = c.before.is_some().then(|| c.path.clone());
                        let new_path = c.after.is_some().then(|| c.path.clone());
                        FileDiff::between(
                            old_path,
                            new_path,
                            c.before.as_deref().unwrap_or_default(),
                            c.after.as_deref().unwrap_or_default(),
                        )
                    })
                    .collect();
                self.repo.diff.set_files(files);
                self.repo.skipped = repo.skipped;
                self.repo.truncated = repo.truncated;
                self.repo.listing = Listing::Ready;
            }
            Err(why) => self.repo.listing = Listing::Failed(why),
        }
        true
    }
}

/// The `git` scene: the example changes as if already written, uncommitted, plus a binary file.
pub(crate) fn scene(s: &mut KrampstaApp) {
    use krampsta_agent::{RepoChange, RepoDiff};

    let changes = crate::sample::changes()
        .into_iter()
        .map(|c| RepoChange { path: c.path, before: c.before, after: Some(c.after) })
        .collect();
    let repo = RepoDiff { changes, skipped: vec!["assets/logo.png".into()], truncated: false };
    s.repo.ask += 1;
    s.land_repo(vec![RepoDiffList { ask: s.repo.ask, result: Ok(repo) }]);
    s.inspector = Inspector::Git;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::{MockCatalog, MockConfig, RepoChange, RepoDiff};

    use super::*;

    fn app() -> KrampstaApp {
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())))
    }

    #[test]
    fn the_last_answer_lands_as_a_diff_with_new_and_deleted_files() {
        let mut app = app();
        app.repo.ask = 2;
        let change = |path: &str, before: Option<&str>, after: Option<&str>| RepoChange {
            path: path.into(),
            before: before.map(Into::into),
            after: after.map(Into::into),
        };
        let repo = RepoDiff {
            changes: vec![
                change("src/main.rs", Some("a\n"), Some("b\n")),
                change("new.rs", None, Some("new\n")),
                change("gone.rs", Some("old\n"), None),
            ],
            skipped: vec!["logo.png".into()],
            truncated: false,
        };
        assert!(!app.land_repo(vec![RepoDiffList { ask: 1, result: Ok(RepoDiff::default()) }]), "stale");
        assert!(app.land_repo(vec![RepoDiffList { ask: 2, result: Ok(repo) }]));
        assert_eq!(app.repo.listing, Listing::Ready);
        let files = app.repo.diff.files();
        assert_eq!(files.len(), 3);
        assert_eq!((files[1].old_path.as_deref(), files[1].new_path.as_deref()), (None, Some("new.rs")));
        assert_eq!((files[2].old_path.as_deref(), files[2].new_path.as_deref()), (Some("gone.rs"), None));
        assert_eq!(app.repo.skipped, ["logo.png"]);
    }

    #[test]
    fn a_failure_says_why_and_a_hidden_view_reads_again_when_shown() {
        let mut app = app();
        app.repo.ask = 1;
        let failed = Err("The project folder isn't in a Git repository".into());
        app.land_repo(vec![RepoDiffList { ask: 1, result: failed }]);
        assert!(matches!(&app.repo.listing, Listing::Failed(why) if why.contains("Git repository")));

        app.repo_changed();
        assert_eq!(app.repo.listing, Listing::Stale);
        app.show_inspector(Inspector::Git);
        assert_eq!(app.repo.listing, Listing::Loading);
    }
}
