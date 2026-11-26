//! Earlier turns' tool traffic as it goes back to the model.
//!
//! A turn's calls and results live in its assistant message ([`Message::tool_calls`],
//! [`Message::tool_results`]). Sending them again lets the model build on what it already read
//! instead of reading it all again; how much of each goes back depends on the turn's age: the
//! last [`RECENT_TURNS`] turns keep up to [`RECENT_OUTPUT_MAX`] bytes of each result, older ones
//! a line or so ([`OLD_OUTPUT_MAX`]), which says what was looked at and that it can be looked
//! at again, and turns older than [`FORGET_AFTER`] only their text. A recent turn spends at most
//! [`RECENT_TURN_MAX`] on results, so one busy turn can't push the conversation out of
//! [`HISTORY_BUDGET`], past which the engines drop whole turns.
//!
//! A [`MessageRole::Summary`] stands for everything before it ([`start`]): the host writes one
//! when the conversation nears the budget (`compact.rs`), and the engines send it, as
//! instructions headed by [`SUMMARY_HEADER`], instead of the messages it covers.

use std::borrow::Cow;

use krampsta_core::{Message, MessageRole, ToolCall};

/// Soft byte budget for what one request sends, not a tokenizer or a model context limit.
pub const HISTORY_BUDGET: usize = 128 * 1024;

/// Heads a summary as the engines send it.
pub const SUMMARY_HEADER: &str = "Summary of the conversation so far (the messages it sums up aren't shown to you; \
the user still sees them):";

/// Turns before the current one whose results go back nearly whole.
pub const RECENT_TURNS: usize = 2;
/// The most of one result a recent turn sends again.
pub const RECENT_OUTPUT_MAX: usize = 8 * 1024;
/// The most of all its results a recent turn sends again; the calls past it go as old ones.
pub const RECENT_TURN_MAX: usize = 32 * 1024;
/// Turns before the current one that send their calls at all; older ones send only their text.
pub const FORGET_AFTER: usize = 8;
/// The most of one result an older turn sends again.
pub const OLD_OUTPUT_MAX: usize = 400;
/// An older turn's arguments past this (an `edit_file`'s text, say) go as a note instead.
pub const OLD_ARGUMENTS_MAX: usize = 1024;

/// One call of an earlier turn and what it returned, both shortened for its age.
#[derive(Debug, PartialEq, Eq)]
pub struct Replayed<'a> {
    pub call: &'a ToolCall,
    /// Always JSON, as providers require.
    pub arguments: Cow<'a, str>,
    pub output: Cow<'a, str>,
}

/// Where the conversation the model is sent starts: the last summary, which stands for what
/// came before it, or the first message. System messages before it still go.
pub fn start(history: &[Message]) -> usize {
    history.iter().rposition(|m| m.role == MessageRole::Summary).unwrap_or(0)
}

/// About how many bytes `history` takes as the engines send it (from [`start`], earlier calls
/// shortened by age), before their JSON framing.
pub fn estimated_bytes(history: &[Message]) -> usize {
    let from = start(history);
    history
        .iter()
        .zip(ages(history))
        .enumerate()
        .filter(|(index, (m, _))| *index >= from || m.role == MessageRole::System)
        .map(|(_, (m, age))| {
            let calls: usize = if m.role == MessageRole::Assistant {
                calls(m, age).map(|r| 96 + r.call.name.len() + r.arguments.len() + r.output.len()).sum()
            } else {
                0
            };
            48 + m.content.len() + calls
        })
        .sum()
}

/// How many user turns came after each message of `history` (0 for the current prompt and
/// anything after it). An answer to the previous question is 1.
pub fn ages(history: &[Message]) -> Vec<usize> {
    let mut ages = vec![0; history.len()];
    let mut later_users = 0;
    for (age, message) in ages.iter_mut().zip(history).rev() {
        *age = later_users;
        if message.role == MessageRole::User {
            later_users += 1;
        }
    }
    ages
}

/// The calls `message` made that finished, in order, with what each returned. A call with no
/// result (its run was stopped) is left out: providers expect every call they see answered.
pub fn calls(message: &Message, age: usize) -> impl Iterator<Item = Replayed<'_>> {
    let calls = if age > FORGET_AFTER { &[][..] } else { &message.tool_calls[..] };
    let mut recent_left = if age <= RECENT_TURNS { RECENT_TURN_MAX } else { 0 };
    calls.iter().filter_map(move |call| {
        let result = message.result_for(call)?;
        let size = result.output.len().min(RECENT_OUTPUT_MAX);
        let (arguments, output) = if size <= recent_left {
            recent_left -= size;
            (Cow::Borrowed(call.arguments.as_str()), shorten(&result.output, RECENT_OUTPUT_MAX))
        } else {
            (shorten_arguments(&call.arguments), shorten(&result.output, OLD_OUTPUT_MAX))
        };
        Some(Replayed { call, arguments, output })
    })
}

/// Whether an earlier assistant message, `age` turns back, has anything to send again.
pub fn has_content(message: &Message, age: usize) -> bool {
    !message.content.is_empty()
        || (age <= FORGET_AFTER && message.tool_calls.iter().any(|call| message.result_for(call).is_some()))
}

/// `text` up to about `max` bytes, cut at a line where one is near, with a note saying how much
/// was left out.
fn shorten(text: &str, max: usize) -> Cow<'_, str> {
    if text.len() <= max {
        return Cow::Borrowed(text);
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &text[..cut];
    let head = match head.rfind('\n') {
        Some(line) if line >= cut / 2 => &head[..line],
        _ => head,
    };
    let left_out = text.len() - head.len();
    Cow::Owned(format!(
        "{head}\n[{left_out} more bytes from an earlier turn left out; run the tool again if you need them]"
    ))
}

fn shorten_arguments(arguments: &str) -> Cow<'_, str> {
    if arguments.len() <= OLD_ARGUMENTS_MAX {
        return Cow::Borrowed(arguments);
    }
    let note = format!("{} bytes of arguments from an earlier turn left out", arguments.len());
    Cow::Owned(serde_json::json!({ "left_out": note }).to_string())
}

#[cfg(test)]
mod tests {
    use krampsta_core::{MessageId, RunId, ToolCallId, ToolResult};

    use super::*;

    fn answered(id: u64, calls: &[(&str, &str, Option<&str>)]) -> Message {
        let mut message = Message::assistant(MessageId(id), RunId(id));
        for (call, arguments, output) in calls {
            let call_id = ToolCallId((*call).into());
            message.tool_calls.push(ToolCall {
                id: call_id.clone(),
                name: "read_file".into(),
                arguments: (*arguments).into(),
            });
            if let Some(output) = output {
                message.tool_results.push(ToolResult { call_id, output: (*output).into(), is_error: false });
            }
        }
        message
    }

    #[test]
    fn ages_count_the_user_turns_that_came_after() {
        let history = [
            Message::user(MessageId(1), "a"),
            answered(2, &[]),
            Message::user(MessageId(3), "b"),
            answered(4, &[]),
            Message::user(MessageId(5), "now"),
        ];
        assert_eq!(ages(&history), [2, 2, 1, 1, 0]);
    }

    #[test]
    fn unanswered_calls_are_left_out_and_order_is_kept() {
        let message = answered(1, &[("a", "{}", Some("one")), ("b", "{}", None), ("c", "{}", Some("three"))]);
        let replayed: Vec<_> = calls(&message, 1).map(|r| (r.call.id.0.as_str(), r.output)).collect();
        assert_eq!(replayed, [("a", Cow::Borrowed("one")), ("c", Cow::Borrowed("three"))]);
        assert!(has_content(&message, 1));
        assert!(!has_content(&answered(2, &[("x", "{}", None)]), 1));
        assert!(!has_content(&message, FORGET_AFTER + 1), "too old: only text, and it has none");
    }

    #[test]
    fn recent_turns_keep_more_than_old_ones() {
        let long = "line\n".repeat(4 * 1024);
        let big_arguments = format!("{{\"text\":\"{}\"}}", "x".repeat(4 * 1024));
        let message = answered(1, &[("a", &big_arguments, Some(&long))]);

        let recent = calls(&message, RECENT_TURNS).next().unwrap();
        assert_eq!(recent.arguments, big_arguments);
        assert!(recent.output.len() < RECENT_OUTPUT_MAX + 128 && recent.output.len() > RECENT_OUTPUT_MAX / 2);
        assert!(recent.output.ends_with("run the tool again if you need them]"));
        assert!(recent.output.starts_with("line\nline\n"));

        let old = calls(&message, RECENT_TURNS + 1).next().unwrap();
        assert!(old.output.len() < OLD_OUTPUT_MAX + 128, "{}", old.output.len());
        let note: serde_json::Value = serde_json::from_str(&old.arguments).unwrap();
        assert!(note["left_out"].as_str().unwrap().starts_with(&big_arguments.len().to_string()));
    }

    #[test]
    fn a_busy_recent_turn_sends_its_first_results_whole_and_the_rest_short() {
        let output = "y".repeat(RECENT_OUTPUT_MAX);
        let many: Vec<_> = (0..10).map(|n| (format!("c{n}"), output.clone())).collect();
        let spec: Vec<_> = many.iter().map(|(id, out)| (id.as_str(), "{}", Some(out.as_str()))).collect();
        let message = answered(1, &spec);
        let sizes: Vec<usize> = calls(&message, 1).map(|r| r.output.len()).collect();
        assert_eq!(sizes[..4], [RECENT_OUTPUT_MAX; 4]);
        assert!(sizes[4..].iter().all(|&size| size < OLD_OUTPUT_MAX + 128), "{sizes:?}");
        assert!(sizes.iter().sum::<usize>() < RECENT_TURN_MAX + 6 * (OLD_OUTPUT_MAX + 128));
    }

    #[test]
    fn a_summary_hides_what_came_before_it_from_the_estimate() {
        let mut history = vec![
            Message::new(MessageId(1), MessageRole::System, "rules"),
            Message::user(MessageId(2), "x".repeat(10_000)),
            Message::new(MessageId(3), MessageRole::Summary, "short"),
            Message::user(MessageId(4), "now"),
        ];
        assert_eq!(start(&history), 2);
        let summed = estimated_bytes(&history);
        assert!(summed < 300, "{summed}");
        history.remove(2);
        assert_eq!(start(&history), 0);
        assert!(estimated_bytes(&history) > 10_000);
    }

    #[test]
    fn short_text_is_borrowed_and_cuts_respect_characters() {
        assert!(matches!(shorten("short", 10), Cow::Borrowed("short")));
        let text = "ñ".repeat(100);
        let cut = shorten(&text, 51);
        assert!(cut.starts_with(&"ñ".repeat(25)));
        assert!(cut.contains("[150 more bytes"));
    }
}
