//! What differs from the repository's last commit, for the right panel's "Git" view: the files
//! `git status` reports (staged, unstaged, untracked), each with what `HEAD` holds and what the
//! folder holds now. It runs the user's `git`, read-only, with no console window.
//!
//! Only the project folder's part of the repository counts (the project may be a subfolder),
//! and paths are relative to the project. A binary file, or one too big to compare, is named but
//! not compared, and the whole read stops at a cap, so the UI's diffs stay cheap.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use krampsta_agent::{RepoChange, RepoDiff};

use crate::project::Project;

/// The biggest file compared, either side, in bytes.
const MAX_FILE: usize = 512 * 1024;
/// The most files compared.
const MAX_FILES: usize = 200;
/// The most text compared in all, both sides, in bytes.
const MAX_TOTAL: usize = 4 * 1024 * 1024;

pub fn repo_diff(project: &Project) -> Result<RepoDiff, String> {
    let root = project.root();
    // Where the project sits in the repository: `git status` gives paths from the repo's root.
    let prefix = text(git(root, &["rev-parse", "--show-prefix"])?).trim().to_owned();
    let has_head = git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
    let status =
        git(root, &["-c", "core.quotepath=off", "status", "--porcelain=v1", "-z", "--untracked-files=all", "--", "."])?;
    let mut diff = RepoDiff::default();
    let mut total = 0;
    for entry in parse_status(&status) {
        let Some(path) = entry.path.strip_prefix(prefix.as_str()) else { continue };
        if diff.changes.len() + diff.skipped.len() == MAX_FILES {
            diff.truncated = true;
            break;
        }
        // What the last commit holds (under its old name, for a rename).
        let before = match (has_head, entry.untracked) {
            (true, false) => {
                git(root, &["show", &format!("HEAD:{}", entry.old_path.as_deref().unwrap_or(&entry.path))]).ok()
            }
            _ => None,
        };
        let after = if entry.deleted { None } else { std::fs::read(root.join(path)).ok() };
        if before.is_none() && after.is_none() {
            continue;
        }
        let size = before.as_ref().map_or(0, Vec::len) + after.as_ref().map_or(0, Vec::len);
        let big =
            before.as_ref().is_some_and(|b| b.len() > MAX_FILE) || after.as_ref().is_some_and(|a| a.len() > MAX_FILE);
        let binary = before.as_deref().is_some_and(is_binary) || after.as_deref().is_some_and(is_binary);
        if big || binary || total + size > MAX_TOTAL {
            diff.skipped.push(path.to_owned());
            continue;
        }
        total += size;
        let shared = |bytes: Vec<u8>| -> Arc<str> { text(bytes).into() };
        diff.changes.push(RepoChange { path: path.to_owned(), before: before.map(shared), after: after.map(shared) });
    }
    Ok(diff)
}

/// One line of `git status --porcelain=v1 -z`.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    path: String,
    /// A rename's or copy's old name.
    old_path: Option<String>,
    untracked: bool,
    deleted: bool,
}

/// `XY path\0` entries; a rename or copy carries its old name in the next field.
fn parse_status(output: &[u8]) -> Vec<Entry> {
    let mut fields = output.split(|b| *b == 0).filter(|f| !f.is_empty());
    let mut entries = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let (x, y) = (field[0], field[1]);
        let path = String::from_utf8_lossy(&field[3..]).into_owned();
        let old_path =
            matches!(x, b'R' | b'C').then(|| fields.next().map(|f| String::from_utf8_lossy(f).into_owned())).flatten();
        entries.push(Entry { path, old_path, untracked: x == b'?', deleted: x == b'D' || y == b'D' });
    }
    entries
}

/// Like Git, a file with a NUL byte near its start is binary.
fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Runs `git args` in `dir`; its stdout, or why it failed in words a person can act on.
fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashes up behind the app.
        command.creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0);
    }
    let output = command.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "Git isn't installed (or isn't on PATH)".to_owned(),
        _ => format!("Git couldn't run: {e}"),
    })?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let said = String::from_utf8_lossy(&output.stderr);
    if said.contains("not a git repository") {
        return Err("The project folder isn't in a Git repository".into());
    }
    Err(format!("git {} failed: {}", args.first().copied().unwrap_or_default(), said.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
    }

    /// A repository with one commit, in a temporary folder.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run(root, &["init", "-q"]);
        run(root, &["config", "user.email", "test@example.com"]);
        run(root, &["config", "user.name", "Test"]);
        run(root, &["config", "commit.gpgsign", "false"]);
        std::fs::create_dir_all(root.join("app/src")).unwrap();
        std::fs::write(root.join("app/src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("app/gone.txt"), "bye\n").unwrap();
        std::fs::write(root.join("app/old.txt"), "moved\n").unwrap();
        std::fs::write(root.join("outside.txt"), "not the project's\n").unwrap();
        run(root, &["add", "."]);
        run(root, &["commit", "-q", "-m", "first"]);
        dir
    }

    #[test]
    fn it_reads_what_differs_from_head_within_the_project() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("app/src/main.rs"), "fn main() { println!(\"hi\"); }\n").unwrap();
        std::fs::remove_file(root.join("app/gone.txt")).unwrap();
        std::fs::write(root.join("app/new.rs"), "pub fn new() {}\n").unwrap();
        std::fs::write(root.join("app/data.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(root.join("outside.txt"), "changed, but outside the project\n").unwrap();
        run(root, &["mv", "app/old.txt", "app/renamed.txt"]);

        let project = Project::open(root.join("app")).unwrap();
        let mut diff = repo_diff(&project).unwrap();
        diff.changes.sort_by(|a, b| a.path.cmp(&b.path));
        let paths: Vec<&str> = diff.changes.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["gone.txt", "new.rs", "renamed.txt", "src/main.rs"], "paths are the project's");
        let by = |p: &str| diff.changes.iter().find(|c| c.path == p).unwrap();
        assert_eq!(by("gone.txt").after, None, "deleted");
        assert_eq!(by("new.rs").before, None, "untracked");
        assert_eq!(by("renamed.txt").before.as_deref(), Some("moved\n"), "a rename compares with its old name");
        assert!(by("src/main.rs").after.as_deref().unwrap().contains("println"));
        assert_eq!(diff.skipped, ["data.bin"], "binary: named, not compared");
        assert!(!diff.truncated);
    }

    #[test]
    fn a_clean_repository_has_no_changes_and_a_plain_folder_says_so() {
        let dir = repo();
        let diff = repo_diff(&Project::open(dir.path()).unwrap()).unwrap();
        assert!(diff.changes.is_empty() && diff.skipped.is_empty());

        let plain = tempfile::tempdir().unwrap();
        let error = repo_diff(&Project::open(plain.path()).unwrap()).unwrap_err();
        assert!(error.contains("isn't in a Git repository"), "{error}");
    }

    #[test]
    fn status_lines_parse_renames_deletions_and_untracked_files() {
        let output = b" M src/a.rs\0R  new.rs\0old.rs\0 D gone.rs\0?? fresh.rs\0";
        let entries = parse_status(output);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[1].old_path.as_deref(), Some("old.rs"));
        assert!(entries[2].deleted && entries[3].untracked);
    }
}
