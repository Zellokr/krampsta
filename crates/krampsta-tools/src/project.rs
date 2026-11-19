//! The project folder the tools work in, and the one rule they share: no path leaves it.

use std::io;
use std::path::{Component, Path, PathBuf};

/// The project's root, resolved once (symlinks followed), so every check is a prefix compare.
#[derive(Clone, Debug)]
pub struct Project {
    root: PathBuf,
}

impl Project {
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotADirectory, format!("{} is not a folder", root.display())));
        }
        Ok(Project { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `path` (relative to the root, or absolute inside it) as a real path inside the project.
    /// Symlinks are followed before the check, so a link can't lead out of the folder either.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let path = path.trim();
        let asked = Path::new(if path.is_empty() { "." } else { path });
        let joined = if asked.is_absolute() { asked.to_path_buf() } else { self.root.join(asked) };
        let real = joined.canonicalize().map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => format!("`{path}` does not exist in the project"),
            _ => format!("could not open `{path}`: {e}"),
        })?;
        if real.starts_with(&self.root) {
            Ok(real)
        } else {
            Err(format!("`{path}` is outside the project folder; only files inside it can be used"))
        }
    }

    /// `path` as a file that may not exist yet, inside the project: `..` is resolved by hand
    /// (never above the root), then the deepest folder that does exist is resolved for real, so a
    /// symlink can't lead out either.
    pub fn resolve_for_write(&self, path: &str) -> Result<PathBuf, String> {
        let path = path.trim();
        if path.is_empty() {
            return Err("the path is empty".into());
        }
        let asked = Path::new(path);
        let joined = if asked.is_absolute() { asked.to_path_buf() } else { self.root.join(asked) };
        let outside = || format!("`{path}` is outside the project folder; only files inside it can be changed");
        let mut clean = PathBuf::with_capacity(joined.as_os_str().len());
        for part in joined.components() {
            match part {
                Component::CurDir => {}
                Component::ParentDir => {
                    if !clean.pop() {
                        return Err(outside());
                    }
                }
                other => clean.push(other),
            }
        }
        // The names below the deepest existing folder, innermost first.
        let mut missing = Vec::new();
        let mut existing = clean.as_path();
        while !existing.exists() {
            missing.push(existing.file_name().ok_or_else(outside)?.to_owned());
            existing = existing.parent().ok_or_else(outside)?;
        }
        let mut real = existing.canonicalize().map_err(|e| format!("could not open `{path}`: {e}"))?;
        if !real.starts_with(&self.root) {
            return Err(outside());
        }
        real.extend(missing.iter().rev());
        if real == self.root {
            return Err(format!("`{path}` is the project folder itself, not a file"));
        }
        Ok(real)
    }

    /// How a path inside the project is shown to the model: relative, with `/` on every system
    /// (`.` for the root itself), so it can be passed back to any tool as written.
    pub fn show(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.root).unwrap_or(path);
        let mut out = String::with_capacity(relative.as_os_str().len());
        for part in relative.components() {
            if let Component::Normal(name) = part {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(&name.to_string_lossy());
            }
        }
        if out.is_empty() {
            out.push('.');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_inside_and_are_refused_outside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let p = Project::open(dir.path()).unwrap();

        let main = p.resolve("src/main.rs").unwrap();
        assert_eq!(p.show(&main), "src/main.rs");
        assert_eq!(p.show(&p.resolve("").unwrap()), ".");
        assert_eq!(p.show(&p.resolve("src/../src/./main.rs").unwrap()), "src/main.rs");
        assert_eq!(p.resolve(&main.to_string_lossy()).unwrap(), main, "absolute inside is fine");

        assert!(p.resolve("..").unwrap_err().contains("outside the project"));
        assert!(p.resolve("src/../../").unwrap_err().contains("outside the project"));
        assert!(p.resolve("nope.txt").unwrap_err().contains("does not exist"));
    }

    #[test]
    fn new_files_resolve_inside_and_never_above_the_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let p = Project::open(dir.path()).unwrap();
        assert_eq!(p.show(&p.resolve_for_write("src/new/mod.rs").unwrap()), "src/new/mod.rs");
        assert_eq!(p.show(&p.resolve_for_write("./a/../b.txt").unwrap()), "b.txt");
        assert!(p.resolve_for_write("../escape.txt").unwrap_err().contains("outside"));
        assert!(p.resolve_for_write("src/../../escape.txt").unwrap_err().contains("outside"));
        assert!(p.resolve_for_write(".").unwrap_err().contains("project folder itself"));
        assert!(p.resolve_for_write("  ").is_err());
    }
}
