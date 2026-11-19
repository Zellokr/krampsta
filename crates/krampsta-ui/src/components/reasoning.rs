//! The model's reasoning above its reply, folded: a header that shimmers "Thinking" while the
//! model is still thinking and says "Reasoning" after, and opens the text as muted Markdown.

use krampsta_core::Message;
use rune::{El, col, icon, row, text};
use rune_kit::markdown::MarkdownStyle;
use rune_kit::{Kit, Markdown};

use crate::components::{icons, shimmer};
use crate::keys;
use crate::state::KrampstaApp;
use crate::theme;

/// `streaming`: the message's run is still writing it. `open`: the user opened the block.
pub fn view(m: &Message, streaming: bool, open: bool, k: &Kit) -> El {
    // Thinking until the model acts or answers.
    let thinking = streaming && m.content.is_empty() && m.tool_calls.is_empty();
    let key = keys::reasoning(m.id);
    let title = if thinking {
        shimmer::view(&format!("{key}-label"), "Thinking", k.ty.ui, k.c.text_faint, k.c.text_base)
    } else {
        text("Reasoning").font(k.ty.ui).color(k.c.text_muted)
    };
    let chevron = if open { &icons::CHEVRON_DOWN } else { &icons::CHEVRON_RIGHT };
    let id = m.id;
    let header = row()
        .key(key)
        .role("button")
        .label(if thinking { "Thinking" } else { "Reasoning" })
        .expanded(open)
        .focusable(true)
        .focus_ring(2.0, 0.0, k.c.border_focus)
        .h(28.0)
        .px(6.0)
        .ml(-6.0)
        .gap(6.0)
        .items_center()
        .radius(k.radius.sm)
        .hover(k.c.hover)
        .on_press(move |s: &mut KrampstaApp| s.toggle_reasoning(id))
        .child(icon(&icons::BRAIN, 14.0).color(k.c.icon_muted))
        .child(title)
        .child(icon(chevron, 14.0).color(k.c.icon_weaker));
    let mut out = col().w_full().gap(4.0).child(row().child(header));
    if open {
        let style = MarkdownStyle::from_kit(k).text(k.c.text_muted).body_font(k.ty.ui);
        let body = Markdown::new(m.reasoning.clone())
            .style(style)
            .streaming(thinking)
            .reveal(theme::WORD_FADE)
            .selectable(true)
            .build(k);
        out = out.child(col().w_full().pl(12.0).border_sides([0.0, 0.0, 0.0, 2.0], k.c.border_weak).child(body));
    }
    out
}
