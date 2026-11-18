//! The agent status pill: a dot (or spinner while running) and the status name.

use krampsta_core::AgentStatus;
use rune::{El, el, row, text};
use rune_kit::{Kit, Spinner};

use crate::keys;
use crate::theme;

pub fn pill(status: &AgentStatus, k: &Kit) -> El {
    let mark = if status.is_running() {
        Spinner::new().size(12.0).color(theme::accent(k.dark)).build(k)
    } else {
        el().size(8.0, 8.0).radius(4.0).fill(theme::status_color(k, status))
    };
    let label = status.label();
    row()
        .key(keys::STATUS)
        .role("status")
        .label(format!("Agent {}", label.to_lowercase()))
        .h(26.0)
        .px(10.0)
        .gap(7.0)
        .items_center()
        .radius(13.0)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_weak)
        .child(row().size(12.0, 12.0).center().child(mark))
        .child(text(label).font(k.ty.small_medium).color(k.c.text_muted))
}
