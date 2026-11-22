//! Disposable source tree for checking pending edits. This is not a process sandbox.

use std::fs;
use std::path::Path;

use krampsta_agent::CancellationToken;

use crate::edit::Pending;
use crate::project::Project;

pub struct Snapshot {
    pub project: Project,
    pub edits: usize,
    _dir: tempfile::TempDir,
}

impl Snapshot {
    pub fn create(project: &Project, pending: &Pending, cancel: &CancellationToken) -> Result<Self, String> {
        check_cancel(cancel)?;
        let edits = pending.snapshot();
        if edits.is_empty() {
            return Err("there are no pending edits to check".into());
        }
        let dir = tempfile::Builder::new().prefix("krampsta-proposal-").tempdir().map_err(|e| e.to_string())?;
        let copy = Project::open(dir.path()).map_err(|e| e.to_string())?;
        let root = project.root();
        if copy.root().starts_with(root) {
            return Err("the temporary directory must be outside the project".into());
        }
        let mut walk = ignore::WalkBuilder::new(root);
        walk.hidden(false).git_global(false).require_git(false).filter_entry(|entry| {
            entry.depth() == 0
                || (entry.file_name() != ".git" && !(entry.depth() == 1 && entry.file_name() == "target"))
        });
        for entry in walk.build().skip(1) {
            check_cancel(cancel)?;
            let entry = entry.map_err(|e| e.to_string())?;
            let relative = entry.path().strip_prefix(root).map_err(|e| e.to_string())?;
            let dest = copy.root().join(relative);
            let meta = fs::symlink_metadata(entry.path()).map_err(|e| copy_error(relative, &e))?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    return Err(format!("cannot copy reparse point `{}`", relative.display()));
                }
            }
            if meta.is_dir() {
                fs::create_dir_all(dest).map_err(|e| copy_error(relative, &e))?;
            } else if meta.is_file() {
                fs::copy(entry.path(), dest).map_err(|e| copy_error(relative, &e))?;
            } else {
                return Err(format!("cannot copy special file `{}`", relative.display()));
            }
        }
        let count = edits.len();
        for (path, content) in edits {
            check_cancel(cancel)?;
            let resolved = project.resolve_for_write(&path)?;
            if project.show(&resolved) != path || resolved.is_dir() {
                return Err(format!("pending path `{path}` no longer points to the original file"));
            }
            let relative = resolved.strip_prefix(root).map_err(|e| e.to_string())?;
            if relative.starts_with("target") || relative.components().any(|p| p.as_os_str() == ".git") {
                return Err(format!("cannot check pending edit to `{}`", relative.display()));
            }
            let dest = copy.root().join(relative);
            // Under the copy's root, so it always has a parent.
            fs::create_dir_all(dest.parent().unwrap_or(copy.root()))
                .and_then(|()| fs::write(&dest, content.as_bytes()))
                .map_err(|e| copy_error(relative, &e))?;
        }
        Ok(Self { project: copy, edits: count, _dir: dir })
    }
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), String> {
    if cancel.is_cancelled() { Err("stopped".into()) } else { Ok(()) }
}

fn copy_error(path: &Path, error: &std::io::Error) -> String {
    format!("could not copy `{}` into the proposal: {error}", path.display())
}
