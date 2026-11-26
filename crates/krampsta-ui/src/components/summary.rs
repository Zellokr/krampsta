//! Where the conversation was summed up for the model (`compact.rs` in `krampsta-agent`): a
//! folded header across the transcript that opens the summary as muted Markdown. The messages
//! above it stay; the model gets the summary instead of them.

use krampsta_core::Message;
use rune::{El, col, icon, row, text};
use rune_kit::markdown::MarkdownStyle;
use rune_kit::{Kit, Markdown};

use crate::components::icons;
use crate::keys;
use crate::state::KrampstaApp;

pub const TITLE: &str = "Earlier messages summed up";
pub const HINT: &str = "From here on the model gets this summary instead of the messages above.";

/// `open`: the user opened the summary.
pub fn view(m: &Message, open: bool, k: &Kit) -> El {
    let chevron = if open { &icons::CHEVRON_DOWN } else { &icons::CHEVRON_RIGHT };
    let id = m.id;
    let header = row()
        .key(keys::summary(id))
        .role("button")
        .label(TITLE)
        .expanded(open)
        .focusable(true)
        .focus_ring(2.0, 0.0, k.c.border_focus)
        .h(28.0)
        .px(6.0)
        .gap(6.0)
        .items_center()
        .radius(k.radius.sm)
        .hover(k.c.hover)
        // Opened and closed like a reasoning block: both are folded text of one message.
        .on_press(move |s: &mut KrampstaApp| s.toggle_reasoning(id))
        .child(icon(&icons::ARCHIVE, 14.0).color(k.c.icon_muted))
        .child(text(TITLE).font(k.ty.ui).color(k.c.text_muted))
        .child(icon(chevron, 14.0).color(k.c.icon_weaker));
    let rule = || col().grow(1.0).h(1.0).fill(k.c.border_weak);
    let line = row().w_full().gap(8.0).items_center().child(rule()).child(header).child(rule());
    let mut out = col().role("group").label(TITLE).w_full().gap(6.0).child(line);
    if open {
        let style = MarkdownStyle::from_kit(k).text(k.c.text_muted).body_font(k.ty.ui);
        let body = Markdown::new(m.content.clone()).style(style).selectable(true).build(k);
        out = out
            .child(text(HINT).font(k.ty.small).color(k.c.text_faint).wrap())
            .child(col().w_full().pl(12.0).border_sides([0.0, 0.0, 0.0, 2.0], k.c.border_weak).child(body));
    }
    out
}
