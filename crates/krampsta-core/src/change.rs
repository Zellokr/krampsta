//! Changes the agent proposes to files, and the line arithmetic for taking part of one.

use std::ops::Range;
use std::sync::Arc;

/// A file's proposed content beside what is on disk. Nothing is written until the user accepts
/// it. The texts are shared (`Arc<str>`): a change is passed around and redrawn without copying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the project root, with `/` separators.
    pub path: String,
    /// What the file holds now; `None` when the change creates it.
    pub before: Option<Arc<str>>,
    pub after: Arc<str>,
}

impl FileChange {
    /// The text on disk, empty for a file the change creates.
    pub fn before_text(&self) -> &str {
        self.before.as_deref().unwrap_or("")
    }

    /// Whether accepting it would change nothing.
    pub fn is_noop(&self) -> bool {
        self.before.as_deref() == Some(&*self.after)
    }
}

/// `target` with its lines `target_lines` replaced by `source`'s lines `source_lines` (0-based
/// line indexes, end exclusive), each line copied with its own line break, so no line ending is
/// rewritten. One pass over both texts.
///
/// Accepting one block of a diff is `splice_lines(before, its old lines, after, its new lines)`;
/// rejecting it is the same the other way round.
pub fn splice_lines(target: &str, target_lines: Range<usize>, source: &str, source_lines: Range<usize>) -> String {
    let t = byte_range(target, target_lines);
    let s = byte_range(source, source_lines);
    let mut out = String::with_capacity(target.len() - t.len() + s.len());
    out.push_str(&target[..t.start]);
    out.push_str(&source[s]);
    out.push_str(&target[t.end..]);
    out
}

/// The bytes of lines `lines` of `text`, line breaks included. Lines past the end are empty.
fn byte_range(text: &str, lines: Range<usize>) -> Range<usize> {
    let start = skip_lines(text, lines.start);
    // Counted on from `start`, so the text before it is scanned once, not twice.
    let end = start + skip_lines(&text[start..], lines.end.saturating_sub(lines.start));
    start..end
}

/// Where `text`'s line `count` starts: the byte after its `count`-th line break (its end when it
/// has fewer).
fn skip_lines(text: &str, count: usize) -> usize {
    match count {
        0 => 0,
        n => text.match_indices('\n').nth(n - 1).map_or(text.len(), |(i, _)| i + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = "a\r\nb\r\nc\r\nd\r\n";
    const AFTER: &str = "a\r\nB\r\nc\r\nd\r\nE\r\n";

    #[test]
    fn a_block_is_taken_with_its_own_line_breaks() {
        // Accept the change to line 2 alone: lines 1..2 of both sides.
        assert_eq!(splice_lines(BEFORE, 1..2, AFTER, 1..2), "a\r\nB\r\nc\r\nd\r\n");
        // Accept the added last line alone: nothing replaced, one line inserted at the end.
        assert_eq!(splice_lines(BEFORE, 4..4, AFTER, 4..5), "a\r\nb\r\nc\r\nd\r\nE\r\n");
        // Reject the change to line 2 from the proposal.
        assert_eq!(splice_lines(AFTER, 1..2, BEFORE, 1..2), "a\r\nb\r\nc\r\nd\r\nE\r\n");
    }

    #[test]
    fn edges_without_a_final_line_break_and_empty_files() {
        assert_eq!(splice_lines("x\ny", 1..2, "x\nz", 1..2), "x\nz");
        assert_eq!(splice_lines("", 0..0, "new\n", 0..1), "new\n");
        assert_eq!(splice_lines("only\n", 0..1, "", 0..0), "");
    }

    #[test]
    fn a_change_knows_when_it_changes_nothing() {
        let c = |before: Option<&str>, after: &str| FileChange {
            path: "a".into(),
            before: before.map(Into::into),
            after: after.into(),
        };
        assert!(c(Some("x"), "x").is_noop());
        assert!(!c(None, "").is_noop(), "creating an empty file is a change");
        assert_eq!(c(None, "y").before_text(), "");
    }
}
