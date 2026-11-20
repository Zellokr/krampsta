//! A tool call in the transcript, as a card: the tool, its main argument and how it went in a
//! header that folds the arguments and output away. A failed call keeps its error in sight.

use krampsta_core::{ToolCall, ToolResult};
use rune::{El, col, icon, row, text};
use rune_kit::diff::FileDiff;
use rune_kit::{Kit, Spinner};

use crate::components::{icons, inline_diff};
use crate::keys;
use crate::state::KrampstaApp;
use crate::theme;

/// Output lines an open card shows; the rest is cut with an ellipsis.
const OUTPUT_LINES: usize = 40;
/// Arguments longer than this (a whole file to write) aren't parsed for the header.
const SUMMARY_SOURCE_MAX: usize = 2048;
/// The argument keys that name what a call is about, most telling first.
const SUMMARY_KEYS: [&str; 6] = ["command", "pattern", "query", "url", "path", "file"];

/// How a call stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Running,
    Done,
    Failed,
}

impl Outcome {
    fn of(result: Option<&ToolResult>) -> Outcome {
        match result {
            None => Outcome::Running,
            Some(r) if r.is_error => Outcome::Failed,
            Some(_) => Outcome::Done,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Outcome::Running => "running",
            Outcome::Done => "done",
            Outcome::Failed => "failed",
        }
    }
}

pub fn view(
    call: &ToolCall,
    result: Option<&ToolResult>,
    live: Option<&str>,
    open: bool,
    pending: bool,
    change: Option<&FileDiff>,
    k: &Kit,
) -> El {
    let outcome = Outcome::of(result);
    let border = if outcome == Outcome::Failed { k.c.danger.icon } else { k.c.border_weak };
    let mut card =
        col().w_full().radius(k.radius.md).fill(k.c.layer_01).border(1.0, border).child(header(call, outcome, open, k));
    if let Some(r) = result.filter(|_| outcome == Outcome::Failed && !open) {
        let first = r.output.lines().next().unwrap_or_default().to_owned();
        card = card.child(text(first).font(k.ty.small).color(k.c.danger.icon).truncate().px(12.0).pb(10.0));
    }
    if pending {
        card = card.child(
            text("Commands read files from disk. Unapplied changes were present when this command was requested.")
                .font(k.ty.small)
                .color(k.c.text_muted)
                .wrap()
                .px(12.0)
                .pb(10.0),
        );
    }
    // A running command shows where it is: its last lines, open or not.
    if let Some(live) = live.filter(|l| result.is_none() && !l.trim().is_empty()) {
        let tail = text(last_lines(live, LIVE_LINES).to_owned()).font(k.ty.mono).color(k.c.text_muted).wrap_anywhere();
        let pane = col().w_full().px(10.0).py(8.0).radius(k.radius.sm).fill(k.c.layer_02).child(tail);
        card = card.child(col().key(keys::tool_live(&call.id)).w_full().px(12.0).pb(12.0).child(pane));
    }
    // The change a call proposed shows in its card, open or not: what the agent did, at a glance.
    if let Some(diff) = change {
        card = card
            .child(col().key(keys::tool_diff(&call.id)).w_full().px(12.0).pb(12.0).child(inline_diff::view(diff, k)));
    }
    if open {
        card = card.child(body(call, result, k));
    }
    card
}

/// Lines of live output a running call shows.
const LIVE_LINES: usize = 8;

/// The last `n` lines of `text`, found from its end: the cost is the tail, not the whole text.
fn last_lines(text: &str, n: usize) -> &str {
    let text = text.trim_end_matches(['\n', '\r']);
    let start = text.rmatch_indices('\n').nth(n.saturating_sub(1)).map_or(0, |(i, _)| i + 1);
    &text[start..]
}

fn header(call: &ToolCall, outcome: Outcome, open: bool, k: &Kit) -> El {
    let status = match outcome {
        Outcome::Running => Spinner::new().size(12.0).color(theme::accent(k.dark)).build(k),
        Outcome::Done => icon(&icons::CHECK, 14.0).color(k.c.icon_muted),
        Outcome::Failed => icon(&icons::WARNING, 14.0).color(k.c.danger.icon),
    };
    let id = call.id.clone();
    let mut title = row()
        .gap(8.0)
        .items_center()
        .shrink()
        .min_w(0.0)
        .child(icon(&icons::TOOL, 14.0).color(k.c.icon_muted))
        .child(text(call.name.clone()).font(k.ty.ui_medium).color(k.c.text_base));
    if let Some(s) = summary(&call.arguments) {
        title = title.child(text(s).font(k.ty.mono).color(k.c.text_muted).truncate().shrink().min_w(0.0));
    }
    row()
        .key(keys::tool(&call.id))
        .role("button")
        .label(format!("Tool {} {}", call.name, outcome.word()))
        .expanded(open)
        .focusable(true)
        .focus_ring(2.0, 0.0, k.c.border_focus)
        .w_full()
        .h(40.0)
        .px(12.0)
        .gap(12.0)
        .items_center()
        .justify_between()
        .radius(k.radius.md)
        .hover(k.c.hover)
        .on_press(move |s: &mut KrampstaApp| s.toggle_tool(&id))
        .child(title)
        .child(status)
}

fn body(call: &ToolCall, result: Option<&ToolResult>, k: &Kit) -> El {
    let mut body =
        col().w_full().gap(10.0).px(12.0).pb(12.0).child(block(k, "Arguments", call.arguments.clone(), None));
    if let Some(r) = result {
        let ink = r.is_error.then_some(k.c.danger.icon);
        body = body.child(block(k, if r.is_error { "Error" } else { "Output" }, clipped(&r.output), ink));
    }
    body
}

/// A labelled monospaced block, selectable so it can be copied.
fn block(k: &Kit, label: &str, content: String, ink: Option<rune::Color>) -> El {
    let content = if content.is_empty() { "(empty)".to_owned() } else { content };
    col().w_full().gap(4.0).child(text(label.to_owned()).font(k.ty.small_medium).color(k.c.text_faint)).child(
        col().w_full().px(10.0).py(8.0).radius(k.radius.sm).fill(k.c.layer_02).child(
            text(content)
                .font(k.ty.mono)
                .color(ink.unwrap_or(k.c.text_base))
                .wrap_anywhere()
                .selectable(true)
                .selection_color(k.selection()),
        ),
    )
}

/// The first [`OUTPUT_LINES`] lines of `output`, with "…" when there is more.
fn clipped(output: &str) -> String {
    let mut lines = output.lines();
    let kept: Vec<&str> = lines.by_ref().take(OUTPUT_LINES).collect();
    let mut out = kept.join("\n");
    if lines.next().is_some() {
        out.push_str("\n…");
    }
    out
}

/// What the call is about, for its header: the first telling string argument (`path`,
/// `command`…), or the only string one. `None` when the arguments say nothing short.
fn summary(arguments: &str) -> Option<String> {
    if arguments.len() > SUMMARY_SOURCE_MAX {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let object = value.as_object()?;
    let telling = SUMMARY_KEYS.iter().find_map(|key| object.get(*key).and_then(|v| v.as_str()));
    let only = || {
        let mut strings = object.values().filter_map(|v| v.as_str());
        match (strings.next(), strings.next()) {
            (Some(s), None) => Some(s),
            _ => None,
        }
    };
    let line = telling.or_else(only)?.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_output_shows_its_last_lines() {
        assert_eq!(last_lines("a\nb\nc\nd\n", 2), "c\nd");
        assert_eq!(last_lines("only", 8), "only");
        assert_eq!(last_lines("a\r\nb\r\n", 1), "b");
    }

    #[test]
    fn the_summary_picks_the_telling_argument() {
        assert_eq!(summary(r#"{"path": "src/main.rs", "limit": 20}"#).as_deref(), Some("src/main.rs"));
        assert_eq!(summary(r#"{"timeout": 5, "command": "cargo test"}"#).as_deref(), Some("cargo test"));
        assert_eq!(
            summary(r#"{"path": "src", "pattern": "fn total"}"#).as_deref(),
            Some("fn total"),
            "what, not where"
        );
        assert_eq!(summary(r#"{"name": "x"}"#).as_deref(), Some("x"), "the only string");
        assert_eq!(summary(r#"{"a": "x", "b": "y"}"#), None, "two strings, neither telling");
        assert_eq!(summary(r#"{"command": "echo a\necho b"}"#).as_deref(), Some("echo a"), "first line");
        assert_eq!(summary("not json"), None);
        assert_eq!(summary(&format!(r#"{{"path": "{}"}}"#, "a".repeat(3000))), None, "too long to parse");
    }

    #[test]
    fn long_output_is_clipped_with_an_ellipsis() {
        let long: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let shown = clipped(&long);
        assert_eq!(shown.lines().count(), OUTPUT_LINES + 1);
        assert!(shown.ends_with('…'));
        assert_eq!(clipped("a\nb"), "a\nb");
    }
}
