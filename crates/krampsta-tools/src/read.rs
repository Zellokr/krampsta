//! `read_file`: a file's lines, numbered, streamed only as far as the range asked for.
//!
//! Cost: one pass over the bytes up to the last line shown, through one reused line buffer;
//! nothing past the range is read, and a huge file never sits in memory.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufRead, BufReader};

use krampsta_agent::CancellationToken;
use serde::Deserialize;

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
    path: String,
    /// The first line to show, counting from 1.
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

pub fn run(project: &Project, args: Args, cancel: &CancellationToken) -> Result<String, String> {
    let path = project.resolve(&args.path)?;
    let shown = project.show(&path);
    if path.is_dir() {
        return Err(format!("`{shown}` is a folder; use list_files to see what it holds"));
    }
    let file = File::open(&path).map_err(|e| format!("could not open `{shown}`: {e}"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let head = reader.fill_buf().map_err(|e| format!("could not read `{shown}`: {e}"))?;
    if head.is_empty() {
        return Ok(format!("`{shown}` is empty."));
    }
    if head[..head.len().min(SNIFF)].contains(&0) {
        return Err(format!("`{shown}` is a binary file; it can't be shown as text"));
    }

    let first = args.offset.unwrap_or(1).max(1);
    let limit = args.limit.unwrap_or(DEFAULT_LINES).max(1);
    let mut out = String::with_capacity(4096);
    let mut line = Vec::with_capacity(256);
    let (mut number, mut last) = (0usize, 0usize);
    let mut more = false;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).map_err(|e| format!("could not read `{shown}`: {e}"))?;
        if read == 0 {
            break;
        }
        number += 1;
        if number % 4096 == 0 && cancel.is_cancelled() {
            return Err("stopped".into());
        }
        if number < first {
            continue; // Skipped lines are never decoded.
        }
        if number - first == limit || out.len() >= MAX_BYTES {
            more = true;
            break;
        }
        let text = String::from_utf8_lossy(trim_newline(&line));
        let _ = write!(out, "{number:>6}\t");
        push_cut(&mut out, &text, MAX_LINE_CHARS);
        out.push('\n');
        last = number;
    }
    if last == 0 {
        return Err(format!("`{shown}` has {number} lines; offset {first} is past its end"));
    }
    if more {
        let _ = write!(out, "\n(Lines {first}–{last} of `{shown}`; there is more. Read on with offset {}.)", last + 1);
    }
    Ok(out)
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
        run(p, Args { path: path.into(), offset, limit }, &CancellationToken::new())
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
