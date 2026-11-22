//! `list_files`: what a folder holds, as `.gitignore` leaves it (build output and dependencies
//! out), depth first in name order, stopping at a cap.
//!
//! Cost: one directory read per folder visited, each sorted by name; the walk ends as soon as the
//! cap is reached, so a huge tree costs no more than the entries shown.

use std::collections::BTreeMap;
use std::path::Path;

use ignore::WalkBuilder;
use krampsta_agent::{CancellationToken, ProjectFiles};
use serde::Deserialize;

use crate::edit::Pending;
use crate::overlay::{Overlay, VirtualIgnores};
use crate::project::Project;

/// The most entries one call lists.
pub const MAX_ENTRIES: usize = 1000;

#[derive(Deserialize)]
pub struct Args {
    #[serde(default)]
    path: Option<String>,
    /// How many levels down to go (1: only the folder's own entries). All of them by default.
    #[serde(default)]
    depth: Option<usize>,
}

pub fn run(project: &Project, pending: &Pending, args: Args, cancel: &CancellationToken) -> Result<String, String> {
    let overlay = Overlay::new(project, pending, cancel)?;
    let dir = overlay.resolve(project, args.path.as_deref().unwrap_or("."))?;
    if dir.is_file() || overlay.get(&dir).is_some() {
        return Err(format!("`{}` is a file, not a folder; use read_file to read it", project.show(&dir)));
    }
    let mut out = String::with_capacity(8 * 1024);
    let mut count = 0;
    let mut cut = false;
    let depth = args.depth.map(|d| d.max(1));
    let mut virtual_entries = virtual_entries(project, &overlay, &dir, depth, cancel)?.into_iter().peekable();
    let mut disk = walker(&dir)
        .max_depth(depth)
        .sort_by_file_name(Ord::cmp)
        .build()
        .take_while(|_| !cancel.is_cancelled())
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() != 0)
        .peekable();
    loop {
        if cancel.is_cancelled() {
            return Err("stopped".into());
        }
        let (path, is_dir) = match (disk.peek(), virtual_entries.peek()) {
            (Some(entry), Some((path, _))) if entry.path() == *path => {
                disk.next();
                continue;
            }
            (Some(entry), Some((path, _))) if entry.path() > *path => {
                let Some((path, is_dir)) = virtual_entries.next() else { break };
                (project.show(path), is_dir)
            }
            (Some(_), _) => {
                let Some(entry) = disk.next() else { break };
                (project.show(entry.path()), entry.file_type().is_some_and(|t| t.is_dir()))
            }
            (None, _) => {
                let Some((path, is_dir)) = virtual_entries.next() else { break };
                (project.show(path), is_dir)
            }
        };
        if count == MAX_ENTRIES {
            cut = true;
            break;
        }
        out.push_str(&path);
        if is_dir {
            out.push('/');
        }
        out.push('\n');
        count += 1;
    }
    if count == 0 {
        return Ok(format!("`{}` is empty (or holds only ignored files).", project.show(&dir)));
    }
    if cut {
        out.push_str("\n(Stopped at 1000 entries; list a subfolder or pass a smaller depth.)");
    }
    Ok(out)
}

fn virtual_entries<'a>(
    project: &Project,
    overlay: &'a Overlay,
    dir: &'a Path,
    depth: Option<usize>,
    cancel: &CancellationToken,
) -> Result<BTreeMap<&'a Path, bool>, String> {
    let mut ignores = None;
    let mut entries = BTreeMap::new();
    for (file, _) in overlay.under(dir) {
        if cancel.is_cancelled() {
            return Err("stopped".into());
        }
        for path in file.ancestors().take_while(|p| *p != dir && !p.exists()) {
            let is_dir = path != file;
            if depth.is_none_or(|max| path.components().count() - dir.components().count() <= max)
                && !ignores.get_or_insert_with(|| VirtualIgnores::new(project.root())).skips(path, dir, is_dir, false)
            {
                entries.insert(path, is_dir);
            }
        }
    }
    Ok(entries)
}

/// The most files the file tree lists.
pub const MAX_FILES: usize = 50_000;

/// Every file in the project as `list_files` would show it (`.gitignore` honoured, `.git`
/// skipped), plus the new files the agent proposed that aren't on disk yet, up to [`MAX_FILES`].
pub fn all_files(project: &Project, pending: &Pending) -> ProjectFiles {
    let mut paths: Vec<String> = Vec::new();
    let mut truncated = false;
    for entry in walker(project.root()).build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if paths.len() == MAX_FILES {
            truncated = true;
            break;
        }
        paths.push(project.show(entry.path()));
    }
    let on_disk: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
    let proposed: Vec<String> = pending.snapshot().into_keys().filter(|p| !on_disk.contains(p.as_str())).collect();
    paths.extend(proposed);
    ProjectFiles { paths, truncated }
}

/// A walk that honours `.gitignore` (also outside a git repository), `.ignore` and the global
/// git excludes, shows dotfiles, and never enters `.git`.
pub fn walker(dir: &std::path::Path) -> WalkBuilder {
    let mut walk = WalkBuilder::new(dir);
    walk.hidden(false).require_git(false).filter_entry(|e| e.file_name() != ".git");
    walk
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(p: &Project, path: Option<&str>, depth: Option<usize>) -> Result<String, String> {
        run(p, &Pending::default(), Args { path: path.map(Into::into), depth }, &CancellationToken::new())
    }

    #[test]
    fn listings_are_sorted_skip_ignored_files_and_git_and_respect_depth() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in ["src/bin", "target/debug", ".git"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["src/main.rs", "src/bin/tool.rs", "target/debug/app", ".git/HEAD", "README.md", ".env.example"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let p = Project::open(root).unwrap();

        assert_eq!(
            list(&p, None, None).unwrap(),
            ".env.example\n.gitignore\nREADME.md\nsrc/\nsrc/bin/\nsrc/bin/tool.rs\nsrc/main.rs\n"
        );
        assert_eq!(list(&p, Some("src"), Some(1)).unwrap(), "src/bin/\nsrc/main.rs\n");
        assert!(list(&p, Some("README.md"), None).unwrap_err().contains("is a file"));
        assert!(list(&p, Some("src/bin/../../.."), None).unwrap_err().contains("outside"));
    }

    #[test]
    fn the_file_tree_gets_every_file_and_the_proposed_new_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in ["src", "target", ".git"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["src/main.rs", "target/app", ".git/HEAD", "README.md"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let p = Project::open(root).unwrap();
        let pending = Pending::default();
        pending.set("src/new.rs", Some("fn new() {}".into()));
        pending.set("README.md", Some("changed".into()));

        let mut files = all_files(&p, &pending);
        files.paths.sort();
        assert_eq!(files.paths, [".gitignore", "README.md", "src/main.rs", "src/new.rs"]);
        assert!(!files.truncated);
    }
}
