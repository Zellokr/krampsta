//! `read_file`: a file's lines, numbered, streamed only as far as the range asked for.
//!
//! Each range is streamed independently, with one lookahead line to detect continuation.
//! Batch ranges share a budget for the rendered file text.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufRead, BufReader};

use krampsta_agent::CancellationToken;
use serde::Deserialize;

use crate::edit::Pending;
use crate::project::Project;

/// Lines shown when the model asks for no limit.
pub const DEFAULT_LINES: usize = 2000;
/// The most output one call returns, so a file of long lines can't flood the model's context.
pub const MAX_BYTES: usize = 256 * 1024;
/// A longer line is cut (minified files, data on one line).
pub const MAX_LINE_CHARS: usize = 2000;
/// How much of the file's start is checked for a NUL byte, as git does to call it binary.
const SNIFF: usize = 8 * 1024;

#[derive(Deserialize)]
pub struct Args {
    path: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
    ranges: Option<Vec<Range>>,
}

#[derive(Deserialize)]
struct Range {
    path: String,
    /// The first line to show, counting from 1.
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

impl Args {
    /// Every file the call reads, as the model wrote it.
    pub(crate) fn paths(&self) -> Vec<&str> {
        self.path.iter().map(String::as_str).chain(self.ranges.iter().flatten().map(|r| r.path.as_str())).collect()
    }
}

pub fn run(project: &Project, pending: &Pending, args: Args, cancel: &CancellationToken) -> Result<String, String> {
    let mut budget = MAX_BYTES;
    if let Some(ranges) = args.ranges {
        if args.path.is_some() || args.offset.is_some() || args.limit.is_some() {
            return Err("pass either ranges or path/offset/limit, not both".into());
        }
        if !(1..=32).contains(&ranges.len()) {
            return Err("ranges must contain between 1 and 32 entries".into());
        }
        let mut out = String::new();
        for range in ranges {
            if cancel.is_cancelled() {
                return Err("stopped".into());
            }
            match read_range(project, pending, &range, &mut budget, cancel) {
                Ok(fragment) => {
                    let _ = writeln!(out, "{:?} ({})", fragment.path, fragment.source);
                    if let Some(last) = fragment.last {
                        let _ = writeln!(out, "lines: {}–{last}", fragment.first);
                    } else {
                        out.push_str("lines: none\n");
                    }
                    if let Some(next) = fragment.next {
                        let _ = writeln!(out, "next_offset: {next}");
                    }
                    out.push_str(&fragment.text);
                }
                Err(error) => {
                    let _ = writeln!(out, "{:?}: {error}", range.path);
                }
            }
            out.push('\n');
        }
        return if cancel.is_cancelled() { Err("stopped".into()) } else { Ok(out) };
    }

    let range = Range {
        path: args.path.ok_or("missing field `path` (or pass `ranges`)")?,
        offset: args.offset,
        limit: args.limit,
    };
    let fragment = read_range(project, pending, &range, &mut budget, cancel)?;
    let Fragment { path, first, last, next, mut text, .. } = fragment;
    if let (Some(last), Some(next)) = (last, next) {
        let _ = write!(text, "\n(Lines {first}–{last} of `{path}`; there is more. Read on with offset {next}.)");
    } else if last.is_none() {
        return Ok(format!("`{path}` is empty."));
    }
    Ok(text)
}

struct Fragment {
    path: String,
    source: &'static str,
    first: usize,
    last: Option<usize>,
    next: Option<usize>,
    text: String,
}

fn read_range(
    project: &Project,
    pending: &Pending,
    range: &Range,
    budget: &mut usize,
    cancel: &CancellationToken,
) -> Result<Fragment, String> {
    // A file with a change proposed reads as proposed: the agent sees its own edits.
    if let Ok(real) = project.resolve_for_write(&range.path)
        && let Some(content) = pending.get(&project.show(&real))
    {
        return lines(content.as_bytes(), project.show(&real), "proposal", range, budget, cancel);
    }
    let path = project.resolve(&range.path)?;
    let shown = project.show(&path);
    if path.is_dir() {
        return Err(format!("`{shown}` is a folder; use list_files to see what it holds"));
    }
    let file = File::open(&path).map_err(|e| format!("could not open `{shown}`: {e}"))?;
    lines(BufReader::with_capacity(64 * 1024, file), shown, "disk", range, budget, cancel)
}

/// Stream to the requested range and read at most one extra line to detect continuation.
fn lines(
    mut reader: impl BufRead,
    shown: String,
    source: &'static str,
    range: &Range,
    budget: &mut usize,
    cancel: &CancellationToken,
) -> Result<Fragment, String> {
    if cancel.is_cancelled() {
        return Err("stopped".into());
    }
    let first = range.offset.unwrap_or(1).max(1);
    let mut fragment = Fragment { path: shown, source, first, last: None, next: None, text: String::new() };
    let shown = &fragment.path;
    let head = reader.fill_buf().map_err(|e| format!("could not read `{shown}`: {e}"))?;
    if head.is_empty() {
        return Ok(fragment);
    }
    if head[..head.len().min(SNIFF)].contains(&0) {
        return Err(format!("`{shown}` is a binary file; it can't be shown as text"));
    }

    let limit = range.limit.unwrap_or(DEFAULT_LINES).max(1);
    let mut line = Vec::with_capacity(256);
    let mut rendered = String::new();
    let mut number = 0usize;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).map_err(|e| format!("could not read `{shown}`: {e}"))?;
        if read == 0 {
            break;
        }
        number += 1;
        if number.is_multiple_of(4096) && cancel.is_cancelled() {
            return Err("stopped".into());
        }
        if number < first {
            continue; // Skipped lines are never decoded.
        }
        if number - first == limit || *budget == 0 {
            fragment.next = Some(number);
            break;
        }
        let text = String::from_utf8_lossy(trim_newline(&line));
        rendered.clear();
        let _ = write!(rendered, "{number:>6}\t");
        push_cut(&mut rendered, &text, MAX_LINE_CHARS);
        rendered.push('\n');
        if rendered.len() > *budget {
            fragment.next = Some(number);
            break;
        }
        *budget -= rendered.len();
        fragment.text.push_str(&rendered);
        fragment.last = Some(number);
    }
    if fragment.last.is_none() && fragment.next.is_none() {
        return Err(format!("`{shown}` has {number} lines; offset {first} is past its end"));
    }
    Ok(fragment)
}

fn trim_newline(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Appends `text`, cut after `max` characters with a note of how much was left out.
pub fn push_cut(out: &mut String, text: &str, max: usize) {
    match text.char_indices().nth(max) {
        None => out.push_str(text),
        Some((cut, _)) => {
            out.push_str(&text[..cut]);
            let _ = write!(out, "… ({} more characters)", text[cut..].chars().count());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &[u8])]) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).unwrap();
        }
        let p = Project::open(dir.path()).unwrap();
        (dir, p)
    }

    fn read(p: &Project, path: &str, offset: Option<usize>, limit: Option<usize>) -> Result<String, String> {
        run(
            p,
            &Pending::default(),
            Args { path: Some(path.into()), offset, limit, ranges: None },
            &CancellationToken::new(),
        )
    }

    #[test]
    fn a_proposed_change_reads_as_proposed_even_for_a_new_file() {
        let (_d, p) = project(&[("a.txt", b"old\n")]);
        let pending = Pending::default();
        pending.set("a.txt", Some("new\n".into()));
        pending.set("src/b.txt", Some("born\n".into()));
        let read = |path: &str| {
            run(
                &p,
                &pending,
                Args { path: Some(path.into()), offset: None, limit: None, ranges: None },
                &CancellationToken::new(),
            )
        };
        assert_eq!(read("./a.txt").unwrap(), "     1\tnew\n");
        assert_eq!(read("src/b.txt").unwrap(), "     1\tborn\n");
    }

    #[test]
    fn lines_come_numbered_and_a_range_says_where_to_read_on() {
        let (_d, p) = project(&[("a.txt", b"one\r\ntwo\nthree\nfour\n")]);
        assert_eq!(read(&p, "a.txt", None, None).unwrap(), "     1\tone\n     2\ttwo\n     3\tthree\n     4\tfour\n");
        let part = read(&p, "a.txt", Some(2), Some(2)).unwrap();
        assert!(part.starts_with("     2\ttwo\n     3\tthree\n"), "{part}");
        assert!(part.ends_with("Read on with offset 4.)"), "{part}");
        assert!(read(&p, "a.txt", Some(9), None).unwrap_err().contains("has 4 lines"));
    }

    #[test]
    fn batch_ranges_share_a_budget_and_resume_at_the_first_omitted_line() {
        let content = format!("{}\n", "x".repeat(1000)).repeat(300);
        let (_d, p) = project(&[("a.txt", content.as_bytes())]);
        let args = serde_json::from_str(
            r#"{"ranges":[{"path":"a.txt","limit":200},{"path":"a.txt","offset":201},{"path":"a.txt","offset":300}]}"#,
        )
        .unwrap();
        let out = run(&p, &Pending::default(), args, &CancellationToken::new()).unwrap();
        assert!(out.contains("lines: 1–200\nnext_offset: 201\n"));
        assert!(out.contains("lines: 201–260\nnext_offset: 261\n"));
        assert!(out.contains("lines: none\nnext_offset: 300\n"));
    }

    #[test]
    fn binary_empty_folders_and_long_lines_are_handled() {
        let long = "x".repeat(MAX_LINE_CHARS + 5);
        let (_d, p) = project(&[("bin", b"PK\x03\x04\0\0data"), ("empty", b""), ("long", long.as_bytes())]);
        assert!(read(&p, "bin", None, None).unwrap_err().contains("binary"));
        assert_eq!(read(&p, "empty", None, None).unwrap(), "`empty` is empty.");
        assert!(read(&p, ".", None, None).unwrap_err().contains("is a folder"));
        assert!(read(&p, "long", None, None).unwrap().contains("… (5 more characters)"));
    }

    #[test]
    fn invalid_utf8_is_shown_not_refused() {
        let (_d, p) = project(&[("latin1", b"caf\xe9\n")]);
        assert_eq!(read(&p, "latin1", None, None).unwrap(), "     1\tcaf\u{fffd}\n");
    }
}
