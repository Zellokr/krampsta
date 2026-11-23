//! `search`: lines matching a regex (or a literal string) across the project, as ripgrep finds
//! them: a parallel walk that honours `.gitignore`, SIMD-accelerated line matching, binary files
//! skipped at their first NUL byte, and a global cap on matching lines (or paths in paths-only mode).
//!
//! Each file is streamed once by one thread. Context is collected in the same pass, and only
//! files with hits are sorted.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::WalkState;
use ignore::overrides::OverrideBuilder;
use krampsta_agent::CancellationToken;
use serde::Deserialize;

use crate::edit::Pending;
use crate::list::walker;
use crate::overlay::{Overlay, VirtualIgnores};
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
    #[serde(default)]
    context: usize,
    #[serde(default)]
    paths_only: bool,
}

const MAX_CONTEXT: usize = 10;

struct Hits {
    path: String,
    lines: Vec<(u64, bool, String)>,
}

struct Collect<'a> {
    count: &'a AtomicUsize,
    cancel: &'a CancellationToken,
    context: usize,
    paths_only: bool,
    last_match: Option<u64>,
    lines: Vec<(u64, bool, String)>,
}

impl Collect<'_> {
    fn push(&mut self, number: u64, matched: bool, bytes: &[u8]) {
        let mut text = String::new();
        if !self.paths_only {
            let line = String::from_utf8_lossy(bytes);
            push_cut(&mut text, line.trim_end_matches(['\n', '\r']), MAX_LINE_CHARS);
        }
        self.lines.push((number, matched, text));
    }

    fn within_tail(&self, number: u64) -> bool {
        self.last_match.is_some_and(|last| number - last <= self.context as u64)
    }
}

impl Sink for Collect<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> std::io::Result<bool> {
        if self.cancel.is_cancelled() {
            return Ok(false);
        }
        let number = mat.line_number().ok_or_else(|| std::io::Error::other("search line numbers are disabled"))?;
        if self.count.fetch_add(1, Ordering::Relaxed) >= MAX_MATCHES {
            if self.within_tail(number) {
                self.push(number, false, mat.bytes());
                return Ok(true);
            }
            return Ok(false);
        }
        self.last_match = Some(number);
        self.push(number, true, mat.bytes());
        Ok(!self.paths_only)
    }

    fn context(&mut self, _: &Searcher, ctx: &SinkContext<'_>) -> std::io::Result<bool> {
        let number = ctx.line_number().ok_or_else(|| std::io::Error::other("search line numbers are disabled"))?;
        if self.cancel.is_cancelled() {
            return Ok(false);
        }
        if self.count.load(Ordering::Relaxed) >= MAX_MATCHES && !self.within_tail(number) {
            return Ok(false);
        }
        self.push(number, false, ctx.bytes());
        Ok(true)
    }
}

pub fn run(project: &Project, pending: &Pending, args: Args, cancel: &CancellationToken) -> Result<String, String> {
    if args.pattern.is_empty() {
        return Err("the pattern is empty".into());
    }
    if args.context > MAX_CONTEXT {
        return Err(format!("context must be between 0 and {MAX_CONTEXT}"));
    }
    let context = if args.paths_only { 0 } else { args.context };
    let overlay = Overlay::new(project, pending, cancel)?;
    let base = overlay.resolve(project, args.path.as_deref().unwrap_or("."))?;
    let matcher = RegexMatcherBuilder::new()
        .fixed_strings(args.literal)
        .case_smart(args.case_sensitive.is_none())
        .case_insensitive(args.case_sensitive == Some(false))
        .line_terminator(Some(b'\n'))
        .build(&args.pattern)
        .map_err(|e| format!("the pattern is not a valid regex ({e}); pass literal: true to search plain text"))?;

    let mut overrides = OverrideBuilder::new(&base);
    if let Some(glob) = args.glob.as_deref().filter(|g| !g.trim().is_empty()) {
        overrides.add(glob).map_err(|e| format!("the glob `{glob}` is not valid: {e}"))?;
    }
    let overrides = overrides.build().map_err(|e| format!("the glob is not valid: {e}"))?;
    let found = Mutex::new(Vec::<Hits>::new());
    let count = AtomicUsize::new(0);
    let stopped = || cancel.is_cancelled() || count.load(Ordering::Relaxed) >= MAX_MATCHES;
    let scan = |searcher: &mut Searcher, path: &Path| {
        // Never inside a file of secrets: what it holds would reach the model (`secrets.rs`).
        if crate::secrets::is_secret(path) {
            return;
        }
        let mut sink = Collect {
            count: &count,
            cancel,
            context,
            paths_only: args.paths_only,
            last_match: None,
            lines: Vec::new(),
        };
        let result = match overlay.get(path) {
            Some(content) => searcher.search_reader(&matcher, content.as_bytes(), &mut sink),
            None => searcher.search_path(&matcher, path, &mut sink),
        };
        // An unreadable file is skipped, as ripgrep does by default.
        if result.is_ok() && sink.last_match.is_some() {
            let hits = Hits { path: project.show(path), lines: sink.lines };
            found.lock().unwrap_or_else(PoisonError::into_inner).push(hits);
        }
    };

    let mut walk = walker(&base);
    walk.overrides(overrides.clone());
    let mut scanned = HashSet::new();
    let mut ignores = None;
    let mut virtual_searcher = make_searcher(context);
    for (file, _) in overlay.under(&base) {
        if stopped() {
            break;
        }
        if file.exists() {
            continue;
        }
        if file != base {
            let matched = overrides.matched(file, false);
            if matched.is_ignore()
                || ignores.get_or_insert_with(|| VirtualIgnores::new(project.root())).skips(
                    file,
                    &base,
                    false,
                    matched.is_whitelist(),
                )
            {
                continue;
            }
        }
        scan(&mut virtual_searcher, file);
        scanned.insert(file);
    }
    if base.exists() && !stopped() {
        walk.build_parallel().run(|| {
            let mut searcher = make_searcher(context);
            let scanned = &scanned;
            Box::new(move |entry| {
                if stopped() {
                    return WalkState::Quit;
                }
                if let Ok(entry) = entry
                    && entry.file_type().is_some_and(|t| t.is_file())
                    && !scanned.contains(entry.path())
                {
                    scan(&mut searcher, entry.path());
                }
                WalkState::Continue
            })
        });
    }
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
        if args.paths_only {
            continue;
        }
        let mut previous = None;
        for (number, matched, text) in &hits.lines {
            use std::fmt::Write as _;
            if context > 0 && previous.is_some_and(|n: u64| n + 1 < *number) {
                out.push_str("--\n");
            }
            let marker = if *matched { ':' } else { '-' };
            let _ = writeln!(out, "{number:>6}{marker} {text}");
            previous = Some(*number);
        }
    }
    if count.load(Ordering::Relaxed) >= MAX_MATCHES {
        let unit = if args.paths_only { "matching paths" } else { "matching lines" };
        use std::fmt::Write as _;
        let _ = write!(out, "\n(Stopped at {MAX_MATCHES} {unit}; narrow the pattern, path or glob.)");
    }
    Ok(out)
}

fn make_searcher(context: usize) -> Searcher {
    SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .before_context(context)
        .after_context(context)
        .build()
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
        let mut args = Args {
            pattern: pattern.into(),
            path: None,
            literal: false,
            glob: None,
            case_sensitive: None,
            context: 0,
            paths_only: false,
        };
        f(&mut args);
        run(p, &Pending::default(), args, &CancellationToken::new())
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
    fn overlapping_context_is_printed_once_and_disjoint_groups_are_separated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "before\nhit\nbetween\nhit\nafter\nskipped\nbefore\nhit\nafter\n")
            .unwrap();
        let p = Project::open(dir.path()).unwrap();
        let out = search(&p, "hit", |a| a.context = 1).unwrap();
        assert_eq!(
            out,
            concat!(
                "a.txt\n     1- before\n     2: hit\n     3- between\n     4: hit\n     5- after\n",
                "--\n     7- before\n     8: hit\n     9- after\n",
            )
        );
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
