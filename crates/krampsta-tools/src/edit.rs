//! `edit_file`, which proposes a change instead of writing it, and the write that happens once
//! the user accepts one.
//!
//! An edit replaces one exact, unique piece of text (or creates a file). It builds on the change
//! already proposed for the file, if any, so several edits to one file chain into one proposal,
//! which reads see too. Cost: one read of the file and one linear search and copy.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use krampsta_agent::ToolOutput;
use krampsta_core::FileChange;
use serde::Deserialize;

use crate::project::Project;

/// The largest file an edit takes: bigger ones are data or generated, not code to edit by hand.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// The content proposed for each file not yet accepted or rejected, by its shown path.
#[derive(Default)]
pub struct Pending(Mutex<HashMap<String, Arc<str>>>);

impl Pending {
    pub fn get(&self, path: &str) -> Option<Arc<str>> {
        self.lock().get(path).cloned()
    }

    pub fn set(&self, path: &str, content: Option<Arc<str>>) {
        let mut map = self.lock();
        match content {
            Some(content) => map.insert(path.to_owned(), content),
            None => map.remove(path),
        };
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<str>>> {
        // Every write is one insert or remove: a panic can't leave the map half-changed.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Deserialize)]
pub struct Args {
    path: String,
    /// The exact text to replace; empty to create the file.
    old_string: String,
    new_string: String,
    /// Replace every occurrence instead of requiring exactly one.
    #[serde(default)]
    replace_all: bool,
}

pub fn run(project: &Project, pending: &Pending, args: Args) -> Result<ToolOutput, String> {
    let real = project.resolve_for_write(&args.path)?;
    let shown = project.show(&real);
    if real.is_dir() {
        return Err(format!("`{shown}` is a folder, not a file"));
    }
    let disk = read_text(&real, &shown)?;
    let current = pending.get(&shown).or_else(|| disk.clone());
    let (old, new) = (args.old_string.as_str(), args.new_string.as_str());
    let after = match current.as_deref() {
        None if old.is_empty() => new.to_owned(),
        None => return Err(format!("`{shown}` does not exist; to create it, pass an empty old_string")),
        Some("") if old.is_empty() => new.to_owned(),
        Some(_) if old.is_empty() => {
            return Err(format!("`{shown}` already exists; give old_string, the exact text to replace"));
        }
        Some(text) => replace(text, old, new, args.replace_all).map_err(|why| why.message(&shown))?,
    };
    if current.as_deref() == Some(after.as_str()) {
        return Err("the edit changes nothing: old_string and new_string give the same file".into());
    }
    let after: Arc<str> = Arc::from(after);
    pending.set(&shown, Some(after.clone()));
    let what = if disk.is_none() { "creating" } else { "an edit to" };
    let text = format!(
        "Proposed {what} `{shown}`. Nothing is written until the user accepts it in the changes panel; \
         your next reads and edits of this file see the proposal."
    );
    Ok(ToolOutput { text, change: Some(FileChange { path: shown, before: disk, after }) })
}

/// Why a replacement could not be made.
#[derive(Debug, PartialEq, Eq)]
enum Miss {
    NotFound,
    MoreThanOnce,
}

impl Miss {
    fn message(&self, shown: &str) -> String {
        match self {
            Miss::NotFound => format!(
                "old_string was not found in `{shown}`. It must match the file exactly, spaces and \
                 line breaks included; read the file again and copy the text from it"
            ),
            Miss::MoreThanOnce => format!(
                "old_string appears more than once in `{shown}`; add surrounding lines to make it \
                 unique, or pass replace_all: true to change every occurrence"
            ),
        }
    }
}

/// `text` with `old` replaced by `new`: the one occurrence, or all with `all`. One pass.
fn replace(text: &str, old: &str, new: &str, all: bool) -> Result<String, Miss> {
    if all {
        return if text.contains(old) { Ok(text.replace(old, new)) } else { Err(Miss::NotFound) };
    }
    let mut found = text.match_indices(old).map(|(i, _)| i);
    let at = found.next().ok_or(Miss::NotFound)?;
    if found.next().is_some() {
        return Err(Miss::MoreThanOnce);
    }
    let mut out = String::with_capacity(text.len() - old.len() + new.len());
    out.push_str(&text[..at]);
    out.push_str(new);
    out.push_str(&text[at + old.len()..]);
    Ok(out)
}

/// A text file's content, `None` when it doesn't exist. Binary, non-UTF-8 and huge files are
/// refused: an edit could not keep them intact.
fn read_text(path: &Path, shown: &str) -> Result<Option<Arc<str>>, String> {
    let bytes = match fs::metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("could not open `{shown}`: {e}")),
        Ok(meta) if meta.len() > MAX_FILE_BYTES => {
            return Err(format!("`{shown}` is larger than 8 MB; Krampsta doesn't edit files that big"));
        }
        Ok(_) => fs::read(path).map_err(|e| format!("could not read `{shown}`: {e}"))?,
    };
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(format!("`{shown}` is a binary file; it can't be edited as text"));
    }
    let text =
        String::from_utf8(bytes).map_err(|_| format!("`{shown}` is not UTF-8 text; it can't be edited safely"))?;
    Ok(Some(Arc::from(text)))
}

/// Writes `content` to `path` if the file still holds `expected` (`None`: it must not exist):
/// to a temporary file beside it, then renamed over it, so a crash never leaves half a file.
pub fn write(project: &Project, path: &str, expected: Option<&str>, content: &str) -> Result<(), String> {
    let real = project.resolve_for_write(path)?;
    let shown = project.show(&real);
    let disk = match fs::read(&real) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("could not read `{shown}`: {e}")),
    };
    if disk.as_deref() != expected.map(str::as_bytes) {
        return Err(format!(
            "`{shown}` changed on disk after Krampsta proposed this edit, so it was not written; \
             reject it and ask again"
        ));
    }
    let parent = real.parent().ok_or_else(|| format!("`{shown}` has no folder"))?;
    fs::create_dir_all(parent).map_err(|e| format!("could not create the folder for `{shown}`: {e}"))?;
    let name = real.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    let temp = parent.join(format!(".{name}.krampsta-{}.tmp", std::process::id()));
    let written = fs::File::create(&temp)
        .and_then(|mut f| f.write_all(content.as_bytes()).and_then(|()| f.sync_all()))
        .and_then(|()| fs::rename(&temp, &real));
    written.map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("could not write `{shown}`: {e}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Project, Pending) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\nfn b() {}\nfn a() {}\n").unwrap();
        let p = Project::open(dir.path()).unwrap();
        (dir, p, Pending::default())
    }

    fn edit(p: &Project, pending: &Pending, path: &str, old: &str, new: &str, all: bool) -> Result<ToolOutput, String> {
        let args = Args { path: path.into(), old_string: old.into(), new_string: new.into(), replace_all: all };
        run(p, pending, args)
    }

    #[test]
    fn an_edit_is_proposed_not_written_and_the_next_builds_on_it() {
        let (dir, p, pending) = setup();
        let out = edit(&p, &pending, "a.rs", "fn b() {}", "fn b() { todo() }", false).unwrap();
        let change = out.change.unwrap();
        assert_eq!(change.path, "a.rs");
        assert_eq!(change.before.as_deref(), Some("fn a() {}\nfn b() {}\nfn a() {}\n"));
        assert_eq!(&*change.after, "fn a() {}\nfn b() { todo() }\nfn a() {}\n");
        assert_eq!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap(), "fn a() {}\nfn b() {}\nfn a() {}\n");

        // The second edit sees the first; the change still compares with the disk.
        let change = edit(&p, &pending, "./a.rs", "todo()", "done()", false).unwrap().change.unwrap();
        assert_eq!(change.before.as_deref(), Some("fn a() {}\nfn b() {}\nfn a() {}\n"));
        assert_eq!(&*change.after, "fn a() {}\nfn b() { done() }\nfn a() {}\n");
    }

    #[test]
    fn the_text_must_be_found_exactly_once_unless_all_are_asked_for() {
        let (_d, p, pending) = setup();
        assert!(edit(&p, &pending, "a.rs", "fn c", "x", false).unwrap_err().contains("not found"));
        assert!(edit(&p, &pending, "a.rs", "fn a() {}", "x", false).unwrap_err().contains("more than once"));
        let all = edit(&p, &pending, "a.rs", "fn a() {}", "fn z() {}", true).unwrap().change.unwrap();
        assert_eq!(&*all.after, "fn z() {}\nfn b() {}\nfn z() {}\n");
        assert!(edit(&p, &pending, "a.rs", "fn b() {}", "fn b() {}", false).unwrap_err().contains("changes nothing"));
    }

    #[test]
    fn files_are_created_with_an_empty_old_string_only_when_missing() {
        let (_d, p, pending) = setup();
        let change = edit(&p, &pending, "src/new.rs", "", "pub fn new() {}\n", false).unwrap().change.unwrap();
        assert_eq!((change.path.as_str(), change.before), ("src/new.rs", None));
        assert!(edit(&p, &pending, "a.rs", "", "x", false).unwrap_err().contains("already exists"));
        assert!(edit(&p, &pending, "gone.rs", "x", "y", false).unwrap_err().contains("does not exist"));
        assert!(edit(&p, &pending, "../out.rs", "", "x", false).unwrap_err().contains("outside"));
    }

    #[test]
    fn writes_land_atomically_and_never_over_a_file_that_changed() {
        let (dir, p, _) = setup();
        let original = "fn a() {}\nfn b() {}\nfn a() {}\n";
        write(&p, "a.rs", Some(original), "new\n").unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap(), "new\n");
        assert!(write(&p, "a.rs", Some(original), "again\n").unwrap_err().contains("changed on disk"));
        assert!(write(&p, "a.rs", None, "x").unwrap_err().contains("changed on disk"), "it exists now");

        write(&p, "deep/er/file.txt", None, "hi").unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("deep/er/file.txt")).unwrap(), "hi");
        let leftovers = std::fs::read_dir(dir.path()).unwrap().filter_map(Result::ok);
        assert!(leftovers.into_iter().all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn binary_and_non_utf8_files_are_not_edited() {
        let (dir, p, pending) = setup();
        std::fs::write(dir.path().join("bin"), b"\0\x01").unwrap();
        std::fs::write(dir.path().join("latin1"), b"caf\xe9").unwrap();
        assert!(edit(&p, &pending, "bin", "x", "y", false).unwrap_err().contains("binary"));
        assert!(edit(&p, &pending, "latin1", "caf", "x", false).unwrap_err().contains("not UTF-8"));
    }
}
