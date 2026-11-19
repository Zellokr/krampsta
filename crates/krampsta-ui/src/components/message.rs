//! A message in the transcript: the user's on the right in a bubble, the agent's on the left
//! as Markdown with its tool calls above.

use krampsta_core::{Message, MessageRole};
use rune::{El, col, icon, row, text};
use rune_kit::{Kit, Markdown, Spinner};

use crate::components::{icons, reasoning, shimmer, tool_card};
use crate::keys;
use crate::state::KrampstaApp;
use crate::theme;

/// `streaming`: the message's run is still writing it.
pub fn view(s: &KrampstaApp, m: &Message, streaming: bool, k: &Kit) -> El {
    let body = match m.role {
        MessageRole::User => user(m, k),
        MessageRole::Assistant => assistant(s, m, streaming, k),
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

fn assistant(s: &KrampstaApp, m: &Message, streaming: bool, k: &Kit) -> El {
    let header = row()
        .gap(8.0)
        .items_center()
        .child(icon(&icons::KRAMPSTA, 16.0).color(theme::accent(k.dark)))
        .child(text(MessageRole::Assistant.label()).font(k.ty.ui_medium).color(k.c.text_base));
    let mut out = col().role("group").label("Krampsta").w_full().gap(10.0).child(header);
    if !m.reasoning.is_empty() {
        out = out.child(reasoning::view(m, streaming, s.open_reasoning.contains(&m.id), k));
    }
    if !m.tool_calls.is_empty() {
        let cards = m.tool_calls.iter().map(|c| tool_card::view(c, m.result_for(c), s.open_tools.contains(&c.id), k));
        out = out.child(col().w_full().gap(8.0).children(cards));
    }
    if m.content.is_empty() {
        // With reasoning streaming, its "Thinking" header shows the model is at work instead.
        if streaming && m.reasoning.is_empty() {
            out = out.child(working(k));
        }
    } else {
        out = out.child(Markdown::new(m.content.clone()).streaming(streaming).selectable(true).build(k));
    }
    out
}

/// Waiting for the reply's first words. Not reasoning: that one is the "Thinking" block.
fn working(k: &Kit) -> El {
    row()
        .gap(8.0)
        .items_center()
        .child(Spinner::new().size(12.0).color(theme::accent(k.dark)).build(k))
        .child(shimmer::view(keys::WORKING, "Working…", k.ty.ui, k.c.text_faint, k.c.text_base))
}
