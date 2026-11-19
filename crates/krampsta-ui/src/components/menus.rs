//! The context menus on session rows and messages, and the delete confirmation. Each menu wraps
//! its trigger: right-clicking it (or the Menu key, or Shift+F10 while it has focus) opens it at
//! the press.

use krampsta_core::{Message, Session};
use rune::{El, Tx, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Dialog, Kit, MenuItem, context_menu};

use crate::components::icons;
use crate::keys;
use crate::menus::MenuTarget;
use crate::state::KrampstaApp;

/// `trigger` (a session's row) with its menu: copy the title, delete it.
pub fn session(s: &KrampstaApp, x: &Session, trigger: El, k: &Kit, layer: &mut Vec<El>) -> El {
    let id = x.id;
    let target = MenuTarget::Session(id);
    let trigger = trigger.on_secondary_press(move |s: &mut KrampstaApp, at, _: &mut Tx| s.open_menu(target, at));
    context_menu(keys::session_menu(id), trigger, s.menu_at(target))
        .item(
            MenuItem::new("Copy title")
                .icon(&icons::COPY)
                .on_select_tx(move |s: &mut KrampstaApp, tx: &mut Tx| s.copy_session_title(id, tx)),
        )
        .separator()
        .item(
            MenuItem::new("Delete…")
                .icon(&icons::TRASH)
                .danger(true)
                // A run in progress has to stop first.
                .disabled(x.status.is_running())
                .on_select(move |s: &mut KrampstaApp| s.ask_delete(id)),
        )
        .on_close(|s: &mut KrampstaApp| s.close_menu())
        .build(k, layer)
}

/// `trigger` (a message) with its menu: copy its text.
pub fn message(s: &KrampstaApp, m: &Message, trigger: El, k: &Kit, layer: &mut Vec<El>) -> El {
    let id = m.id;
    let target = MenuTarget::Message(id);
    let trigger = trigger.on_secondary_press(move |s: &mut KrampstaApp, at, _: &mut Tx| s.open_menu(target, at));
    context_menu(keys::message_menu(id), trigger, s.menu_at(target))
        .item(
            MenuItem::new("Copy text")
                .icon(&icons::COPY)
                .disabled(m.content.is_empty())
                .on_select_tx(move |s: &mut KrampstaApp, tx: &mut Tx| s.copy_message(id, tx)),
        )
        .on_close(|s: &mut KrampstaApp| s.close_menu())
        .build(k, layer)
}

/// Asks before deleting a session. Mounted from the root view on any screen.
pub fn delete_dialog(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>) {
    let Some(x) = s.confirm_delete.and_then(|id| s.session(id)) else { return };
    let cancel = Button::new("Cancel")
        .variant(ButtonVariant::Neutral)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut KrampstaApp| s.cancel_delete())
        .build(k)
        .key(keys::DELETE_SESSION_CANCEL);
    let delete = Button::new("Delete")
        .variant(ButtonVariant::Danger)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut KrampstaApp| s.confirm_delete())
        .build(k)
        .key(keys::DELETE_SESSION_CONFIRM);
    let body = rune::col()
        .w_full()
        .gap(16.0)
        .px(16.0)
        .pb(18.0)
        .child(
            text(format!("“{}” and its messages will be gone. This can't be undone.", x.title))
                .font(k.ty.ui)
                .color(k.c.text_muted)
                .wrap(),
        )
        .child(row().w_full().gap(8.0).justify_end().child(cancel).child(delete));
    Dialog::new(keys::DELETE_SESSION, "Delete this session?")
        .open(true)
        .fit()
        .width(400.0)
        .child(body)
        .on_close(|s: &mut KrampstaApp| s.cancel_delete())
        .build(k, layer);
}
