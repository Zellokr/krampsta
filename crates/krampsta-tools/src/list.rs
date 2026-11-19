//! `list_files`: what a folder holds, as `.gitignore` leaves it (build output and dependencies
//! out), depth first in name order, stopping at a cap.
//!
//! Cost: one directory read per folder visited, each sorted by name; the walk ends as soon as the
//! cap is reached, so a huge tree costs no more than the entries shown.

use ignore::WalkBuilder;
use krampsta_agent::CancellationToken;
use serde::Deserialize;

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

pub fn run(project: &Project, args: Args, cancel: &CancellationToken) -> Result<String, String> {
    let dir = project.resolve(args.path.as_deref().unwrap_or("."))?;
    if !dir.is_dir() {
        return Err(format!("`{}` is a file, not a folder; use read_file to read it", project.show(&dir)));
    }
    let mut out = String::with_capacity(8 * 1024);
    let mut count = 0;
    let mut cut = false;
    for entry in walker(&dir).max_depth(args.depth.map(|d| d.max(1))).sort_by_file_name(Ord::cmp).build() {
        if cancel.is_cancelled() {
            return Err("stopped".into());
        }
        // An unreadable entry (permissions, a broken link) is skipped, not fatal.
        let Ok(entry) = entry else { continue };
        if entry.depth() == 0 {
            continue;
        }
        if count == MAX_ENTRIES {
            cut = true;
            break;
        }
        out.push_str(&project.show(entry.path()));
        if entry.file_type().is_some_and(|t| t.is_dir()) {
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
        run(p, Args { path: path.map(Into::into), depth }, &CancellationToken::new())
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
}
