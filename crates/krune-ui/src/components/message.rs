//! A message in the transcript: the user's on the right in a bubble, the agent's on the left
//! as Markdown with its tool calls above.

use krune_core::{Message, MessageRole, ToolCall};
use rune::{El, col, icon, row, text};
use rune_kit::{Kit, Markdown, Spinner};

use crate::components::icons;
use crate::keys;
use crate::theme;

/// `streaming`: the message's run is still writing it.
pub fn view(m: &Message, streaming: bool, k: &Kit) -> El {
    let body = match m.role {
        MessageRole::User => user(m, k),
        MessageRole::Assistant => assistant(m, streaming, k),
        MessageRole::System | MessageRole::Tool => {
            text(m.content.clone()).font(k.ty.small).color(k.c.text_faint).wrap().selectable(true)
        }
    };
    body.key(keys::message(m.id))
}

fn user(m: &Message, k: &Kit) -> El {
    let bubble = col()
        .max_w(600.0)
        .min_w(0.0)
        .px(14.0)
        .py(10.0)
        .radius(k.radius.lg)
        .fill(k.c.layer_02)
        .border(1.0, k.c.border_weak)
        .child(text(m.content.clone()).font(k.ty.body).color(k.c.text_base).wrap().selectable(true));
    row().role("group").label("You").w_full().justify_end().child(bubble)
}

fn assistant(m: &Message, streaming: bool, k: &Kit) -> El {
    let header = row()
        .gap(8.0)
        .items_center()
        .child(icon(&icons::KRUNE, 16.0).color(theme::accent(k.dark)))
        .child(text(MessageRole::Assistant.label()).font(k.ty.ui_medium).color(k.c.text_base));
    let mut out = col().role("group").label("Krune").w_full().gap(10.0).child(header);
    if !m.tool_calls.is_empty() {
        out = out.child(row().gap(6.0).flex_wrap().children(m.tool_calls.iter().map(|c| tool_chip(m, c, k))));
    }
    if m.content.is_empty() {
        if streaming {
            out = out.child(thinking(k));
        }
    } else {
        out = out.child(Markdown::new(m.content.clone()).streaming(streaming).selectable(true).build(k));
    }
    out
}

fn thinking(k: &Kit) -> El {
    row()
        .gap(8.0)
        .items_center()
        .child(Spinner::new().size(12.0).color(theme::accent(k.dark)).build(k))
        .child(text("Thinking…").font(k.ty.ui).color(k.c.text_muted))
}

fn tool_chip(m: &Message, call: &ToolCall, k: &Kit) -> El {
    let (state, ink) = match m.result_for(call) {
        None => ("running", k.c.text_faint),
        Some(r) if r.is_error => ("failed", k.c.danger.icon),
        Some(_) => ("done", k.c.success.icon),
    };
    row()
        .role("note")
        .label(format!("Tool {} {state}", call.name))
        .h(24.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .radius(k.radius.sm)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_weak)
        .child(icon(&icons::TOOL, 12.0).color(k.c.icon_muted))
        .child(text(call.name.clone()).font(k.ty.mono).color(k.c.text_base))
        .child(text(state).font(k.ty.small).color(ink))
}
