//! The composer: a growing text area with a send (or stop) button, and under it a hint line
//! and the provider and model pickers. A pasted image attaches to the draft (`images.rs`).

use krampsta_agent::MOCK_PROVIDER;
use krampsta_core::SessionId;
use rune::{El, TextStyle, Tx, col, el, row, text};
use rune_kit::{IconButton, IconButtonVariant, Kit, Switch};

use crate::components::engine_picker::{self, Place};
use crate::components::{icons, status};
use crate::keys;
use crate::state::KrampstaApp;

/// The longest side of the attached image's preview, in points.
const PREVIEW: f32 = 96.0;

pub fn view(s: &KrampstaApp, session: SessionId, running: bool, k: &Kit, layer: &mut Vec<El>) -> El {
    let draft = s.draft_of(session);
    let view = s.views.get(&session);
    let attachment = view.and_then(|v| v.attachment.as_ref());
    let attaching = view.is_some_and(|v| v.attaching);
    // rune-kit's TextArea has no paste hook, so the field is the plain input inside the same
    // frame `TextArea` draws (`rune-kit/src/text_input.rs`).
    // The line height TextArea gives its text.
    let style = TextStyle { line_height: (13.0f32 * 1.35 * 2.0).round() / 2.0, ..k.ty.ui };
    let input = rune::text_input(draft, style)
        .key(keys::COMPOSER)
        .w_full()
        .label("Message")
        .color(k.c.text_base)
        .caret(k.c.text_base, k.selection())
        .placeholder("Ask Krampsta to explore, explain or change your code…", k.c.text_faint)
        .on_input_paste(move |s: &mut KrampstaApp, clipboard, _| {
            let Some(image) = clipboard.image() else { return false };
            s.focus_session(session);
            s.attach_image(session, image.clone());
            // Text copied along with the image still pastes.
            !clipboard.text().trim().is_empty()
        })
        .on_edit(move |s: &mut KrampstaApp| &mut s.view_mut(session).draft)
        // Typing or sending in a pane makes its session the active one, so the send goes there.
        .on_input_change(move |s: &mut KrampstaApp, _: &mut Tx| {
            s.focus_session(session);
            s.notice = None;
        })
        .on_submit(move |s: &mut KrampstaApp, _: &mut Tx| {
            s.focus_session(session);
            s.submit();
        });
    let mut field = col()
        .role("textbox")
        .w_full()
        .padding(8.0)
        .gap(8.0)
        .radius(k.radius.md)
        .fill(k.c.bg_base)
        .token_motion(k.motion.control)
        .child(rune::auto_grow(input, 2, 10));
    if draft.focused {
        field = field.outline(1.0, -1.0, k.c.border_focus);
    } else {
        for shadow in k.shadow_layers(k.elevation.input()) {
            field = field.box_shadow(shadow);
        }
        field = field.hover(k.c.hover);
    }
    if let Some(image) = attachment {
        field = field.child(attached(&image.thumb, session, k));
    } else if attaching {
        field = field.child(text("Reading the image…").font(k.ty.small).color(k.c.text_faint));
    }

    let action = if running {
        IconButton::new(&icons::STOP)
            .variant(IconButtonVariant::Neutral)
            .label("Stop the agent")
            .help("Stop")
            .on_press(move |s: &mut KrampstaApp| {
                s.focus_session(session);
                s.cancel();
            })
            .build(k)
            .key(keys::STOP)
    } else {
        IconButton::new(&icons::ARROW_UP)
            .variant(IconButtonVariant::Submit)
            .label("Send message")
            .help("Send (Enter)")
            .disabled(attaching || (draft.text.trim().is_empty() && attachment.is_none()))
            .on_press(move |s: &mut KrampstaApp| {
                s.focus_session(session);
                s.submit();
            })
            .build(k)
            .key(keys::SEND)
    };
    // Why a send failed, else why the selection can't run, else how to use the composer.
    // The notice is about the active session: only its pane shows it.
    let notice = s.notice.as_ref().filter(|_| session == s.selected);
    let yolo = s.is_yolo(session);
    let hint = match (notice, &s.engine.warning()) {
        (Some(n), _) => text(n.clone()).font(k.ty.small).color(k.c.danger.icon),
        (None, _) if yolo => text(YOLO_HINT).font(k.ty.small).color(k.c.warning.icon),
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
        .child(row().w_full().items_center().child(yolo_switch(session, yolo, k)).child(el().grow(1.0)).child(pickers))
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

/// Under the field while the session is in yolo mode.
pub const YOLO_HINT: &str = "Yolo mode: tools run and edits are written without asking";

/// Yolo mode for this session (`yolo.rs`), left of the pickers.
fn yolo_switch(session: SessionId, on: bool, k: &Kit) -> El {
    Switch::new(on)
        .label("Yolo")
        .on_toggle(move |s: &mut KrampstaApp| s.toggle_yolo(session))
        .build(k)
        .key(keys::COMPOSER_YOLO)
}

/// The attached image's preview, with a button that removes it.
fn attached(thumb: &rune::Pixels, session: SessionId, k: &Kit) -> El {
    let (width, height) = thumb.size();
    // Shrunk to the preview's size, never enlarged.
    let scale = (PREVIEW / width.max(height).max(1) as f32).min(1.0);
    row()
        .gap(4.0)
        .items_start()
        .child(
            el().key(keys::COMPOSER_IMAGE)
                .role("img")
                .label("Attached image")
                .size(width as f32 * scale, height as f32 * scale)
                .radius(k.radius.sm)
                .clip_rounded(k.radius.sm)
                .pixels(thumb),
        )
        .child(
            IconButton::new(&icons::XMARK)
                .variant(IconButtonVariant::GhostMuted)
                .label("Remove the attached image")
                .help("Remove image")
                .on_press(move |s: &mut KrampstaApp| {
                    s.focus_session(session);
                    s.remove_attachment(session);
                })
                .build(k)
                .key(keys::COMPOSER_IMAGE_REMOVE),
        )
}
