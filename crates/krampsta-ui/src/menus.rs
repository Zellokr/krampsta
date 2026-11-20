//! Context menus (right-click, the Menu key or Shift+F10) on session rows and messages, and
//! deleting a session after confirming it: the state they keep and what their items do.

use krampsta_core::{MessageId, SessionId};
use rune::{PointerPress, Tx};

use crate::state::KrampstaApp;

/// What a context menu was opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuTarget {
    Session(SessionId),
    Message(MessageId),
}

/// An open context menu: what for, and the press it opens at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpenMenu {
    pub target: MenuTarget,
    pub at: PointerPress,
}

impl KrampstaApp {
    pub fn open_menu(&mut self, target: MenuTarget, at: PointerPress) {
        self.menu = Some(OpenMenu { target, at });
    }

    pub fn close_menu(&mut self) {
        self.menu = None;
    }

    /// Where `target`'s menu opens, while it is the open one.
    pub fn menu_at(&self, target: MenuTarget) -> Option<PointerPress> {
        self.menu.filter(|m| m.target == target).map(|m| m.at)
    }

    pub fn copy_session_title(&mut self, id: SessionId, tx: &mut Tx) {
        self.menu = None;
        if let Some(x) = self.session(id) {
            tx.copy(x.title.clone());
        }
    }

    /// Copies a message's text (the Markdown source, for the agent's replies).
    pub fn copy_message(&mut self, id: MessageId, tx: &mut Tx) {
        self.menu = None;
        if let Some(m) = self.sessions.iter().flat_map(|x| &x.messages).find(|m| m.id == id) {
            tx.copy(m.content.clone());
        }
    }

    /// Opens the confirmation for deleting `id`.
    pub fn ask_delete(&mut self, id: SessionId) {
        self.menu = None;
        self.confirm_delete = Some(id);
    }

    pub fn cancel_delete(&mut self) {
        self.confirm_delete = None;
    }

    /// Deletes the session waiting for confirmation. A running session stays (stop it first);
    /// deleting the selected one selects the next, and deleting the last leaves a new one.
    pub fn confirm_delete(&mut self) {
        let Some(id) = self.confirm_delete.take() else { return };
        let Some(index) = self.sessions.iter().position(|x| x.id == id) else { return };
        if self.sessions[index].status.is_running() {
            return;
        }
        let removed = self.sessions.remove(index);
        if let Some(store) = &self.store {
            store.delete(id);
        }
        for m in &removed.messages {
            for c in &m.tool_calls {
                self.open_tools.remove(&c.id);
            }
        }
        if self.sessions.is_empty() {
            self.new_session();
        } else if self.selected == id {
            let next = self.sessions[index.min(self.sessions.len() - 1)].id;
            self.select(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::{MockCatalog, MockConfig};
    use rune::TextEdit;

    use super::*;

    fn app() -> KrampstaApp {
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())))
    }

    const AT: PointerPress = PointerPress { x: 10.0, y: 20.0 };

    #[test]
    fn one_menu_is_open_at_a_time_at_its_press() {
        let mut app = app();
        let (a, b) = (app.sessions[0].id, app.sessions[1].id);
        app.open_menu(MenuTarget::Session(a), AT);
        assert_eq!(app.menu_at(MenuTarget::Session(a)), Some(AT));
        app.open_menu(MenuTarget::Session(b), AT);
        assert_eq!(app.menu_at(MenuTarget::Session(a)), None);
        app.close_menu();
        assert_eq!(app.menu_at(MenuTarget::Session(b)), None);
    }

    #[test]
    fn deleting_the_selected_session_selects_the_next_one() {
        let mut app = app();
        let ids: Vec<SessionId> = app.sessions.iter().map(|x| x.id).collect();
        app.select(ids[0]);
        app.ask_delete(ids[0]);
        assert_eq!(app.confirm_delete, Some(ids[0]));
        app.confirm_delete();
        assert!(app.session(ids[0]).is_none());
        assert_eq!(app.selected, ids[1]);
        assert_eq!(app.confirm_delete, None);
    }

    #[test]
    fn cancelling_keeps_the_session() {
        let mut app = app();
        let id = app.sessions[0].id;
        app.ask_delete(id);
        app.cancel_delete();
        app.confirm_delete();
        assert!(app.session(id).is_some());
    }

    #[test]
    fn deleting_every_session_leaves_a_new_empty_one() {
        let mut app = app();
        for id in app.sessions.iter().map(|x| x.id).collect::<Vec<_>>() {
            app.ask_delete(id);
            app.confirm_delete();
        }
        assert_eq!(app.sessions.len(), 1);
        assert!(app.sessions[0].messages.is_empty());
        assert_eq!(app.selected, app.sessions[0].id);
    }

    #[test]
    fn a_running_session_is_not_deleted() {
        let slow = MockConfig { think: std::time::Duration::from_secs(30), ..MockConfig::default() };
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::new(slow)));
        app.new_session();
        app.draft = TextEdit::new("take your time");
        app.send().unwrap();
        let id = app.selected;
        app.ask_delete(id);
        app.confirm_delete();
        assert!(app.session(id).is_some());
        app.cancel();
    }
}
