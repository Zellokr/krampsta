//! The changes the agent proposed, as the changes panel reviews them: accepting or rejecting a
//! whole file or one block of it. Accepted text is written by the host, off the UI thread; its
//! outcome lands through [`KrampstaApp::pump`].
//!
//! Each change keeps its diff, computed once when the change arrives or moves (Myers, in
//! rune-kit); taking a block is a linear splice of the two texts (`splice_lines`).

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use krampsta_agent::Written;
use krampsta_core::{FileChange, splice_lines};
use rune_kit::diff::{Annotation, FileDiff, Hunk, LineKind, LineRef, Side};

use crate::state::KrampstaApp;

/// The proposals and their diffs, index for index.
#[derive(Default)]
pub struct Changes {
    proposals: Vec<FileChange>,
    files: Vec<FileDiff>,
    /// Paths with a write in flight: their buttons wait.
    writing: HashSet<String>,
    /// Why the last write failed, until the next one.
    pub error: Option<String>,
    validate_after_writes: Option<crate::approvals::Asking>,
}

impl Changes {
    pub fn proposals(&self) -> &[FileChange] {
        &self.proposals
    }

    pub fn is_writing(&self, file: usize) -> bool {
        self.proposals.get(file).is_some_and(|c| self.writing.contains(&c.path))
    }
}

/// The 0-based line ranges a block covers on each side.
fn lines(hunk: &Hunk) -> (Range<usize>, Range<usize>) {
    let range = |start: u32, count: u32| {
        // A side the block adds nothing to names the line it follows (`@@ -3,0 …`).
        let first = if count == 0 { start as usize } else { (start as usize).saturating_sub(1) };
        first..first + count as usize
    };
    (range(hunk.old_start, hunk.old_count), range(hunk.new_start, hunk.new_count))
}

/// The id of a block's actions, shown under its last changed line.
fn block_id(file: usize, hunk: usize) -> String {
    format!("block-{file}-{hunk}")
}

/// The file and block an action's id names.
pub fn parse_block_id(id: &str) -> Option<(usize, usize)> {
    let (file, hunk) = id.strip_prefix("block-")?.split_once('-')?;
    Some((file.parse().ok()?, hunk.parse().ok()?))
}

pub(crate) fn diff_of(change: &FileChange) -> FileDiff {
    let old_path = change.before.is_some().then(|| change.path.clone());
    FileDiff::between(old_path, Some(change.path.clone()), change.before_text(), &change.after)
}

impl KrampstaApp {
    /// Shows a change a tool proposed, replacing the one before it for the same file.
    pub fn propose(&mut self, change: FileChange) {
        if self.stage_proposal(change) {
            self.refresh_diff();
        }
    }

    /// Update the review immediately; the caller refreshes the panel after the batch.
    pub(crate) fn stage_proposal(&mut self, change: FileChange) -> bool {
        match self.review.proposals.iter().position(|c| c.path == change.path) {
            Some(i) => self.replace_change(i, change),
            None if change.is_noop() => return false,
            None => {
                self.review.files.push(diff_of(&change));
                self.review.proposals.push(change);
            }
        }
        true
    }

    pub(crate) fn can_apply_and_validate(&self) -> bool {
        !self.review.proposals.is_empty() && self.review.writing.is_empty() && !self.validating_changes()
    }

    pub(crate) fn validating_changes(&self) -> bool {
        self.review.validate_after_writes.is_some()
    }

    /// Freeze a headless scene mid-write without touching the workspace.
    pub(crate) fn stage_applying_scene(&mut self) {
        if let (Some(asking), Some(change)) = (self.approvals.front(), self.review.proposals.first()) {
            self.review.validate_after_writes = Some(asking.clone());
            self.review.writing.insert(change.path.clone());
        }
    }

    pub(crate) fn apply_and_validate(&mut self) {
        let Some(asking) = self.approvals.front().filter(|a| a.tool == "run_command").cloned() else { return };
        if !self.can_apply_and_validate() {
            return;
        }
        self.review.validate_after_writes = Some(asking);
        for change in self.review.proposals.clone() {
            self.write(&change.path, change.before, change.after);
            if self.review.error.is_some() {
                break;
            }
        }
    }

    pub(crate) fn finish_apply_and_validate(&mut self) -> bool {
        if !self.review.writing.is_empty() {
            return false;
        }
        let Some(asking) = self.review.validate_after_writes.take() else { return false };
        if self.approvals.front() == Some(&asking) {
            if self.review.error.is_none() && self.review.proposals.is_empty() {
                self.answer(krampsta_agent::Decision::Once);
            } else {
                self.review.error.get_or_insert_with(|| "Changes are still pending.".into());
            }
        }
        true
    }

    /// Writes the whole proposal for `file`.
    pub fn accept_file(&mut self, file: usize) {
        if self.validating_changes() {
            return;
        }
        if let Some(c) = self.review.proposals.get(file) {
            let (path, before, after) = (c.path.clone(), c.before.clone(), c.after.clone());
            self.write(&path, before, after);
        }
    }

    /// Drops the proposal for `file`; the agent's next reads see the disk again.
    pub fn reject_file(&mut self, file: usize) {
        if file < self.review.proposals.len() && !self.review.is_writing(file) {
            let change = self.review.proposals.remove(file);
            self.review.files.remove(file);
            self.agent.set_pending(&change.path, None);
            self.refresh_diff();
        }
    }

    /// Writes only block `hunk` of `file`; the rest stays proposed.
    pub fn accept_block(&mut self, file: usize, hunk: usize) {
        let (Some(c), Some(h)) = (self.review.proposals.get(file), self.block(file, hunk)) else { return };
        let (old, new) = lines(h);
        let partial: Arc<str> = splice_lines(c.before_text(), old, &c.after, new).into();
        let (path, before) = (c.path.clone(), c.before.clone());
        self.write(&path, before, partial);
    }

    /// Takes block `hunk` out of `file`'s proposal.
    pub fn reject_block(&mut self, file: usize, hunk: usize) {
        if self.review.is_writing(file) {
            return;
        }
        let (Some(c), Some(h)) = (self.review.proposals.get(file), self.block(file, hunk)) else { return };
        let (old, new) = lines(h);
        let after: Arc<str> = splice_lines(&c.after, new, c.before_text(), old).into();
        let change = FileChange { after, ..c.clone() };
        self.replace_change(file, change);
        self.refresh_diff();
    }

    fn block(&self, file: usize, hunk: usize) -> Option<&Hunk> {
        self.review.files.get(file)?.hunks.get(hunk)
    }

    fn write(&mut self, path: &str, before: Option<Arc<str>>, content: Arc<str>) {
        if !self.review.writing.insert(path.to_owned()) {
            return; // One write per file at a time.
        }
        self.review.error = None;
        if let Err(e) = self.agent.write_file(path, before, content) {
            self.review.writing.remove(path);
            self.review.error = Some(e.to_string());
        }
    }

    /// Applies finished writes: the disk now holds what was written, so the proposal is compared
    /// with that (and goes once nothing is left to accept). Returns (visible change, panel dirty).
    pub(crate) fn land_writes(&mut self, writes: Vec<Written>) -> (bool, bool) {
        let changed = !writes.is_empty();
        let mut panel_dirty = false;
        for Written { path, content, result } in writes {
            self.review.writing.remove(&path);
            match result {
                Err(e) => self.review.error = Some(e),
                Ok(()) => {
                    if let Some(i) = self.review.proposals.iter().position(|c| c.path == path) {
                        let change = FileChange { before: Some(content), ..self.review.proposals[i].clone() };
                        self.replace_change(i, change);
                        panel_dirty = true;
                    }
                }
            }
        }
        (changed, panel_dirty)
    }

    /// Puts `change` at `i`, or drops it when it no longer changes anything, and tells the tools
    /// what the agent's next edits build on.
    fn replace_change(&mut self, i: usize, change: FileChange) {
        if change.is_noop() {
            self.agent.set_pending(&change.path, None);
            self.review.proposals.remove(i);
            self.review.files.remove(i);
        } else {
            self.agent.set_pending(&change.path, Some(change.after.clone()));
            self.review.files[i] = diff_of(&change);
            self.review.proposals[i] = change;
        }
    }

    /// Hands the diffs to the panel, with each block's actions under its last changed line.
    pub(crate) fn refresh_diff(&mut self) {
        let mut notes = Vec::new();
        // A file with one block is accepted from its header; only several blocks need their own.
        for (f, file) in self.review.files.iter().enumerate().filter(|(_, file)| file.hunks.len() > 1) {
            for (h, hunk) in file.hunks.iter().enumerate() {
                let last = hunk.lines.iter().rev().find(|l| l.kind != LineKind::Context);
                let at = last.and_then(|l| match l.kind {
                    LineKind::Removed => l.old.map(|n| LineRef::new(Side::Old, n)),
                    _ => l.new.map(|n| LineRef::new(Side::New, n)),
                });
                if let Some(at) = at {
                    notes.push(Annotation::new(f, at, block_id(f, h)));
                }
            }
        }
        // Cheap: the diffs are already computed, only their lines are copied.
        self.diff.set_files(self.review.files.clone());
        self.diff.set_annotations(notes);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use krampsta_agent::{MockCatalog, ToolOutput, ToolSpec, Toolbox};

    use super::*;

    /// A disk in memory: writes check what they expect, as the real tools do; "locked" refuses.
    #[derive(Default)]
    struct Disk {
        files: Mutex<HashMap<String, String>>,
        pending: Mutex<HashMap<String, Arc<str>>>,
    }

    impl Toolbox for Disk {
        fn specs(&self) -> Arc<[ToolSpec]> {
            Arc::from([])
        }
        fn run(&self, _: &str, _: &str, _: &krampsta_agent::Call<'_>) -> Result<ToolOutput, String> {
            Err("no tools".into())
        }
        fn write(&self, path: &str, expected: Option<&str>, content: &str) -> Result<(), String> {
            let mut files = self.files.lock().unwrap();
            if path == "locked" {
                return Err("permission denied".into());
            }
            if files.get(path).map(String::as_str) != expected {
                return Err("changed on disk".into());
            }
            files.insert(path.into(), content.into());
            Ok(())
        }
        fn set_pending(&self, path: &str, content: Option<Arc<str>>) {
            let mut pending = self.pending.lock().unwrap();
            match content {
                Some(c) => pending.insert(path.into(), c),
                None => pending.remove(path),
            };
        }
    }

    const BEFORE: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n";

    /// Line 2 and line 19 changed: two blocks.
    fn two_blocks() -> String {
        BEFORE.replacen("\n2\n", "\ntwo\n", 1).replacen("\n19\n", "\nnineteen\n", 1)
    }

    fn app(disk: &Arc<Disk>, files: &[(&str, &str)]) -> KrampstaApp {
        for (path, text) in files {
            disk.files.lock().unwrap().insert((*path).into(), (*text).into());
        }
        KrampstaApp::new(Arc::new(MockCatalog::default())).with_tools(disk.clone())
    }

    fn change(path: &str, before: Option<&str>, after: &str) -> FileChange {
        FileChange { path: path.into(), before: before.map(Into::into), after: after.into() }
    }

    fn pump_until_written(app: &mut KrampstaApp) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.review.writing.is_empty() {
            assert!(Instant::now() < deadline, "the write never landed");
            app.pump();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn accepting_one_block_writes_it_and_the_other_stays_proposed() {
        let disk = Arc::new(Disk::default());
        let mut app = app(&disk, &[("a.txt", BEFORE)]);
        app.propose(change("a.txt", Some(BEFORE), &two_blocks()));
        assert_eq!(app.diff.files().len(), 1);
        assert_eq!(app.diff.annotations().len(), 2, "one action row per block");

        app.accept_block(0, 0);
        pump_until_written(&mut app);
        let written = BEFORE.replacen("\n2\n", "\ntwo\n", 1);
        assert_eq!(disk.files.lock().unwrap()["a.txt"], written, "only the first block");
        let left = &app.review.proposals()[0];
        assert_eq!((left.before.as_deref(), &*left.after), (Some(written.as_str()), two_blocks().as_str()));
        assert_eq!(app.diff.annotations().len(), 0, "one block left: its file header acts for it");

        app.accept_file(0);
        pump_until_written(&mut app);
        assert_eq!(disk.files.lock().unwrap()["a.txt"], two_blocks());
        assert!(app.review.proposals().is_empty(), "nothing left to accept");
        assert!(!disk.pending.lock().unwrap().contains_key("a.txt"), "the agent reads the disk again");
    }

    #[test]
    fn rejecting_a_block_takes_it_out_of_the_proposal_and_tells_the_tools() {
        let disk = Arc::new(Disk::default());
        let mut app = app(&disk, &[("a.txt", BEFORE)]);
        app.propose(change("a.txt", Some(BEFORE), &two_blocks()));
        app.reject_block(0, 1);
        let expected = BEFORE.replacen("\n2\n", "\ntwo\n", 1);
        assert_eq!(&*app.review.proposals()[0].after, expected);
        assert_eq!(disk.pending.lock().unwrap().get("a.txt").map(|c| c.to_string()), Some(expected));

        app.reject_block(0, 0);
        assert!(app.review.proposals().is_empty(), "rejecting every block rejects the change");
        assert!(disk.pending.lock().unwrap().is_empty());
        assert_eq!(disk.files.lock().unwrap()["a.txt"], BEFORE, "nothing was written");
    }

    #[test]
    fn new_files_rejected_files_and_failed_writes() {
        let disk = Arc::new(Disk::default());
        let mut app = app(&disk, &[]);
        app.propose(change("new.txt", None, "hello\n"));
        app.propose(change("locked", None, "x\n"));
        app.propose(change("same.txt", Some("x"), "x"));
        assert_eq!(app.review.proposals().len(), 2, "a change that changes nothing is not shown");

        app.accept_file(0);
        pump_until_written(&mut app);
        assert_eq!(disk.files.lock().unwrap()["new.txt"], "hello\n");

        app.accept_file(0); // Now "locked", the only one left.
        pump_until_written(&mut app);
        assert_eq!(app.review.error.as_deref(), Some("permission denied"));
        assert_eq!(app.review.proposals().len(), 1, "a failed write keeps the proposal");

        app.reject_file(0);
        assert!(app.review.proposals().is_empty());
    }

    #[test]
    fn a_later_proposal_for_the_same_file_replaces_the_first() {
        let disk = Arc::new(Disk::default());
        let mut app = app(&disk, &[("a.txt", "a\n")]);
        app.propose(change("a.txt", Some("a\n"), "b\n"));
        app.propose(change("a.txt", Some("a\n"), "c\n"));
        assert_eq!(app.review.proposals().len(), 1);
        assert_eq!(&*app.review.proposals()[0].after, "c\n");
    }

    #[test]
    fn block_actions_follow_the_remaining_file_after_removal() {
        let disk = Arc::new(Disk::default());
        let mut app = app(&disk, &[("a.txt", BEFORE), ("b.txt", BEFORE)]);
        for path in ["a.txt", "b.txt"] {
            app.propose(change(path, Some(BEFORE), &two_blocks()));
        }
        app.reject_file(0);
        assert_eq!(app.diff.files()[0].new_path.as_deref(), Some("b.txt"));
        let actions: Vec<_> = app
            .diff
            .annotations()
            .iter()
            .map(|note| {
                assert_eq!(note.file, 0);
                parse_block_id(&note.id).unwrap()
            })
            .collect();
        assert_eq!(actions, [(0, 0), (0, 1)]);

        app.accept_block(actions[0].0, actions[0].1);
        pump_until_written(&mut app);
        app.reject_block(0, 0);
        assert!(app.review.proposals().is_empty());
        let files = disk.files.lock().unwrap();
        assert_eq!(files["a.txt"], BEFORE);
        assert_eq!(files["b.txt"], BEFORE.replacen("\n2\n", "\ntwo\n", 1));
    }

    #[test]
    fn block_ids_round_trip() {
        assert_eq!(parse_block_id(&block_id(3, 12)), Some((3, 12)));
        assert_eq!(parse_block_id("block-x-1"), None);
        assert_eq!(parse_block_id("note-1-2"), None);
    }

    #[test]
    fn a_block_covers_its_lines_on_each_side() {
        let old: String = (1..=20).map(|n| format!("{n}\n")).collect();
        let new = old.replacen("\n5\n", "\nfive\n", 1) + "21\n";
        let f = FileDiff::from_texts("a", &old, &new);
        assert_eq!(f.hunks.len(), 2, "{:?}", f.hunks);
        // Line 5 changed, with 3 lines of context either side: lines 2–8, 0-based 1..8.
        assert_eq!(lines(&f.hunks[0]), (1..8, 1..8));
        // Line 21 added after 20, with lines 18–20 as context.
        assert_eq!(lines(&f.hunks[1]), (17..20, 17..21));
    }
}
