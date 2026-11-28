//! The context menus on session rows and messages, and the delete confirmation. Each menu wraps
//! its trigger: right-clicking it (or the Menu key, or Shift+F10 while it has focus) opens it at
//! the press.

use krampsta_core::{Message, MessageRole, Session};
use rune::{El, Tx, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Dialog, Kit, MenuItem, context_menu};

use crate::components::icons;
use crate::keys;
use crate::menus::{Confirm, MenuTarget};
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
        .item(MenuItem::new("Rename").icon(&icons::PENCIL).on_select(move |s: &mut KrampstaApp| s.start_rename(id)))
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

/// `trigger` (a message of `x`) with its menu: copy its text and, on the user's own messages,
/// rewind to before it.
pub fn message(s: &KrampstaApp, x: &Session, m: &Message, trigger: El, k: &Kit, layer: &mut Vec<El>) -> El {
    let (id, session) = (m.id, x.id);
    let target = MenuTarget::Message(id);
    let trigger = trigger.on_secondary_press(move |s: &mut KrampstaApp, at, _: &mut Tx| s.open_menu(target, at));
    let mut menu = context_menu(keys::message_menu(id), trigger, s.menu_at(target)).item(
        MenuItem::new("Copy text")
            .icon(&icons::COPY)
            .disabled(m.content.is_empty())
            .on_select_tx(move |s: &mut KrampstaApp, tx: &mut Tx| s.copy_message(id, tx)),
    );
    if m.role == MessageRole::User {
        menu = menu.separator().item(
            MenuItem::new("Rewind to here…")
                .icon(&icons::UNDO)
                // A run in progress has to stop first.
                .disabled(x.status.is_running())
                .on_select(move |s: &mut KrampstaApp| s.ask_rewind(session, id)),
        );
    }
    menu.on_close(|s: &mut KrampstaApp| s.close_menu()).build(k, layer)
}

/// Asks before rewinding a session: what goes, and how many files go back.
pub fn rewind_dialog(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>) {
    let Some(Confirm::Rewind { session, message }) = s.confirm else { return };
    let said = match s.rewind_files(session, message) {
        0 => "Krampsta wrote no files since, so none go back.".to_owned(),
        1 => "The file Krampsta wrote since goes back as it was.".to_owned(),
        n => format!("The {n} files Krampsta wrote since go back as they were."),
    };
    let what = format!(
        "This message and everything after it leave the conversation, and its text returns to the \
         composer. {said} Changes made by commands are not undone."
    );
    let go = |s: &mut KrampstaApp| s.confirm_rewind();
    let ok = ("Rewind", ButtonVariant::Contrast, keys::REWIND_CONFIRM, go as fn(&mut KrampstaApp));
    confirm_dialog(k, layer, (keys::REWIND, "Rewind to this message?", what), ok, keys::REWIND_CANCEL);
}

/// A confirmation: its key, title and text; the button that goes ahead (label, look, key,
/// action); and the cancel button's key. Closing it cancels.
fn confirm_dialog(
    k: &Kit,
    layer: &mut Vec<El>,
    (key, title, what): (&str, &str, String),
    (label, look, ok_key, ok): (&str, ButtonVariant, &str, fn(&mut KrampstaApp)),
    cancel_key: &str,
) {
    let cancel = Button::new("Cancel")
        .variant(ButtonVariant::Neutral)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut KrampstaApp| s.cancel_confirm())
        .build(k)
        .key(cancel_key);
    let go = Button::new(label).variant(look).control_size(ControlSize::Regular).on_press(ok).build(k).key(ok_key);
    let body = rune::col()
        .w_full()
        .gap(16.0)
        .px(16.0)
        .pb(18.0)
        .child(text(what).font(k.ty.ui).color(k.c.text_muted).wrap())
        .child(row().w_full().gap(8.0).justify_end().child(cancel).child(go));
    Dialog::new(key, title)
        .open(true)
        .fit()
        .width(400.0)
        .child(body)
        .on_close(|s: &mut KrampstaApp| s.cancel_confirm())
        .build(k, layer);
}

/// Asks before deleting a session. Mounted from the root view on any screen.
pub fn delete_dialog(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>) {
    let Some(Confirm::Delete(id)) = s.confirm else { return };
    let Some(x) = s.session(id) else { return };
    let what = format!("“{}” and its messages will be gone. This can't be undone.", x.title);
    let go = |s: &mut KrampstaApp| s.confirm_delete();
    let ok = ("Delete", ButtonVariant::Danger, keys::DELETE_SESSION_CONFIRM, go as fn(&mut KrampstaApp));
    confirm_dialog(k, layer, (keys::DELETE_SESSION, "Delete this session?", what), ok, keys::DELETE_SESSION_CANCEL);
}
