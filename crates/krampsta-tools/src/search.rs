//! `search`: lines matching a regex (or a literal string) across the project, as ripgrep finds
//! them: a parallel walk that honours `.gitignore`, SIMD-accelerated line matching, binary files
//! skipped at their first NUL byte, and a global cap that stops every thread once reached.
//!
//! Cost: each file is read once, by one thread, streaming through a fixed buffer; the walk quits
//! when the cap is hit, and only the hits found (at most the cap) are sorted.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::WalkState;
use ignore::overrides::OverrideBuilder;
use krampsta_agent::CancellationToken;
use serde::Deserialize;

use crate::list::walker;
use crate::project::Project;
use crate::read::push_cut;

/// The most matching lines one call returns.
pub const MAX_MATCHES: usize = 200;
/// A longer matching line is cut.
const MAX_LINE_CHARS: usize = 300;

#[derive(Deserialize)]
pub struct Args {
    pattern: String,
    /// The folder (or file) to search; the whole project by default.
    #[serde(default)]
    path: Option<String>,
    /// Match `pattern` as plain text, not a regex.
    #[serde(default)]
    literal: bool,
    /// Only files whose path matches this glob (`*.rs`, `src/**/*.ts`).
    #[serde(default)]
    glob: Option<String>,
    /// Case: `true` exact, `false` ignored; by default ignored unless the pattern has a capital.
    #[serde(default)]
    case_sensitive: Option<bool>,
}

/// One file's matching lines.
struct Hits {
    path: String,
    lines: Vec<(u64, String)>,
}

pub fn run(project: &Project, args: Args, cancel: &CancellationToken) -> Result<String, String> {
    if args.pattern.is_empty() {
        return Err("the pattern is empty".into());
    }
    let base = project.resolve(args.path.as_deref().unwrap_or("."))?;
    let matcher = RegexMatcherBuilder::new()
        .fixed_strings(args.literal)
        .case_smart(args.case_sensitive.is_none())
        .case_insensitive(args.case_sensitive == Some(false))
        .line_terminator(Some(b'\n'))
        .build(&args.pattern)
        .map_err(|e| format!("the pattern is not a valid regex ({e}); pass literal: true to search plain text"))?;

    let mut walk = walker(&base);
    if let Some(glob) = args.glob.as_deref().filter(|g| !g.trim().is_empty()) {
        let mut overrides = OverrideBuilder::new(&base);
        overrides.add(glob).map_err(|e| format!("the glob `{glob}` is not valid: {e}"))?;
        walk.overrides(overrides.build().map_err(|e| format!("the glob `{glob}` is not valid: {e}"))?);
    }

    let found = Mutex::new(Vec::<Hits>::new());
    let count = AtomicUsize::new(0);
    walk.build_parallel().run(|| {
        let (matcher, found, count) = (matcher.clone(), &found, &count);
        let mut searcher = SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build();
        Box::new(move |entry| {
            if cancel.is_cancelled() || count.load(Ordering::Relaxed) >= MAX_MATCHES {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            let mut lines = Vec::new();
            let sink = Lossy(|number, line: &str| {
                if count.fetch_add(1, Ordering::Relaxed) >= MAX_MATCHES {
                    return Ok(false);
                }
                let mut text = String::with_capacity(line.len().min(MAX_LINE_CHARS + 32));
                push_cut(&mut text, line.trim_end_matches(['\n', '\r']), MAX_LINE_CHARS);
                lines.push((number, text));
                Ok(true)
            });
            // An unreadable file is skipped, as ripgrep does by default.
            if searcher.search_path(&matcher, entry.path(), sink).is_ok() && !lines.is_empty() {
                let hits = Hits { path: project.show(entry.path()), lines };
                found.lock().unwrap_or_else(PoisonError::into_inner).push(hits);
            }
            WalkState::Continue
        })
    });
    if cancel.is_cancelled() {
        return Err("stopped".into());
    }

    let mut found = found.into_inner().unwrap_or_else(PoisonError::into_inner);
    if found.is_empty() {
        return Ok(format!("No matches for `{}`.", args.pattern));
    }
    found.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    let mut out = String::with_capacity(found.iter().map(|h| h.path.len() + h.lines.len() * 64).sum());
    for hits in &found {
        out.push_str(&hits.path);
        out.push('\n');
        for (number, text) in &hits.lines {
            use std::fmt::Write as _;
            let _ = writeln!(out, "{number:>6}: {text}");
        }
    }
    if count.load(Ordering::Relaxed) >= MAX_MATCHES {
        out.push_str("\n(Stopped at 200 matching lines; narrow the pattern, path or glob.)");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn main() {\n    run(1 + 2);\n}\n").unwrap();
        std::fs::write(root.join("src/b.ts"), "export function Run() {}\n").unwrap();
        std::fs::write(root.join("target/gen.rs"), "fn run() {}\n").unwrap();
        std::fs::write(root.join("blob.bin"), b"run\0\0\0").unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let p = Project::open(root).unwrap();
        (dir, p)
    }

    fn search(p: &Project, pattern: &str, f: impl FnOnce(&mut Args)) -> Result<String, String> {
        let mut args = Args { pattern: pattern.into(), path: None, literal: false, glob: None, case_sensitive: None };
        f(&mut args);
        run(p, args, &CancellationToken::new())
    }

    #[test]
    fn matches_group_by_file_in_path_order_skipping_ignored_and_binary_files() {
        let (_d, p) = project();
        assert_eq!(
            search(&p, r"\brun\b", |_| {}).unwrap(),
            "src/a.rs\n     2:     run(1 + 2);\nsrc/b.ts\n     1: export function Run() {}\n"
        );
        assert_eq!(search(&p, "Run", |_| {}).unwrap(), "src/b.ts\n     1: export function Run() {}\n", "smart case");
        let exact = search(&p, "run", |a| a.case_sensitive = Some(true)).unwrap();
        assert!(exact.starts_with("src/a.rs") && !exact.contains("b.ts"), "{exact}");
    }

    #[test]
    fn literal_text_globs_and_bad_patterns() {
        let (_d, p) = project();
        assert_eq!(search(&p, "1 + 2", |a| a.literal = true).unwrap(), "src/a.rs\n     2:     run(1 + 2);\n");
        assert_eq!(search(&p, "function", |a| a.glob = Some("*.rs".into())).unwrap(), "No matches for `function`.");
        assert!(search(&p, "run(", |_| {}).unwrap_err().contains("literal: true"));
        assert!(search(&p, "", |_| {}).is_err());
    }

    #[test]
    fn the_cap_stops_the_search() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("many.txt"), "hit\n".repeat(MAX_MATCHES * 3)).unwrap();
        let p = Project::open(dir.path()).unwrap();
        let out = search(&p, "hit", |_| {}).unwrap();
        assert_eq!(out.lines().filter(|l| l.ends_with(": hit")).count(), MAX_MATCHES);
        assert!(out.ends_with("narrow the pattern, path or glob.)"));
    }
}
