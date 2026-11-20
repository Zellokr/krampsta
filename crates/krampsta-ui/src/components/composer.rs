//! The composer: a growing text area with a send (or stop) button, and under it a hint line
//! and the provider and model pickers.

use krampsta_agent::MOCK_PROVIDER;
use krampsta_core::SessionId;
use rune::{El, Tx, col, el, row, text};
use rune_kit::{IconButton, IconButtonVariant, Kit, TextArea};

use crate::components::engine_picker::{self, Place};
use crate::components::{icons, status};
use crate::keys;
use crate::state::KrampstaApp;

pub fn view(s: &KrampstaApp, session: SessionId, running: bool, k: &Kit, layer: &mut Vec<El>) -> El {
    let draft = s.draft_of(session);
    let field = TextArea::new(draft)
        .placeholder("Ask Krampsta to explore, explain or change your code…")
        .label("Message")
        .auto_grow(2, 10)
        .width(None)
        .on_edit(move |s: &mut KrampstaApp| &mut s.view_mut(session).draft)
        .on_change(|s: &mut KrampstaApp, _: &mut Tx| s.notice = None)
        .on_submit(|s: &mut KrampstaApp, _: &mut Tx| s.submit())
        .build(k)
        .key(keys::COMPOSER);

    let action = if running {
        IconButton::new(&icons::STOP)
            .variant(IconButtonVariant::Neutral)
            .label("Stop the agent")
            .help("Stop")
            .on_press(|s: &mut KrampstaApp| {
                s.cancel();
            })
            .build(k)
            .key(keys::STOP)
    } else {
        IconButton::new(&icons::ARROW_UP)
            .variant(IconButtonVariant::Submit)
            .label("Send message")
            .help("Send (Enter)")
            .disabled(draft.text.trim().is_empty())
            .on_press(|s: &mut KrampstaApp| s.submit())
            .build(k)
            .key(keys::SEND)
    };

    // Why a send failed, else why the selection can't run, else how to use the composer.
    let hint = match (&s.notice, &s.engine.warning()) {
        (Some(n), _) => text(n.clone()).font(k.ty.small).color(k.c.danger.icon),
        (None, Some(p)) => text(p.clone()).font(k.ty.small).color(k.c.warning.icon),
        (None, None) if s.engine.selection.provider == MOCK_PROVIDER => {
            text("Enter to send · Shift+Enter for a new line · /fail simulates an error")
                .font(k.ty.small)
                .color(k.c.text_faint)
        }
        (None, None) => text("Enter to send · Shift+Enter for a new line").font(k.ty.small).color(k.c.text_faint),
    };
    // The pickers keep their width; the hint gives way (and truncates) instead.
    let mut pickers = row()
        .gap(2.0)
        .items_center()
        .child(engine_picker::provider(s, k, layer, Place::Composer))
        .child(engine_picker::model(s, k, layer, Place::Composer));
    if s.engine.takes_effort() {
        pickers = pickers.child(engine_picker::effort(s, k, layer, Place::Composer));
    }
    // Pickers above the field, on the right; the hint under it.
    col()
        .w_full()
        .max_w(820.0)
        .gap(8.0)
        .child(row().w_full().items_center().justify_end().child(pickers))
        .child(row().w_full().gap(8.0).items_end().child(col().grow(1.0).min_w(0.0).child(field)).child(action))
        .child(
            row()
                .w_full()
                .gap(12.0)
                .items_center()
                .child(hint.key(keys::COMPOSER_HINT).truncate().shrink().min_w(0.0))
                .child(el().grow(1.0))
                // What the session has used (and cost), out of the toolbar the tabs need.
                .children(s.session(session).and_then(|x| status::usage(x.usage, s.cost(x), k))),
        )
}
