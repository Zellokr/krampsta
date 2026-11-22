//! Renaming a session in place: a double click on its row in the sidebar (or "Rename" in its
//! menu) turns the title into a field (rune-kit's `SidebarRow::renaming`). Return keeps the new
//! name and Escape the old one; a rename left open is kept when another session is chosen, a new
//! one is made or another rename starts. A blank name keeps the old one. The name is saved with
//! the session.

use krampsta_core::SessionId;
use rune::TextEdit;

use crate::state::KrampstaApp;

/// The longest name a session keeps, in characters.
const MAX_TITLE: usize = 200;

/// The session being renamed, and its field.
#[derive(Default)]
pub struct Renaming {
    pub session: Option<SessionId>,
    pub edit: TextEdit,
}

impl KrampstaApp {
    /// Turns `id`'s title into a field, keeping any rename already open.
    pub fn start_rename(&mut self, id: SessionId) {
        self.commit_rename();
        self.menu = None;
        let Some(session) = self.session(id) else { return };
        let mut edit = TextEdit::new(session.title.clone());
        edit.focused = true;
        self.renaming = Renaming { session: Some(id), edit };
        // One field has the caret: the composers give it up meanwhile.
        for view in self.views.values_mut() {
            view.draft.focused = false;
        }
    }

    /// Keeps the name typed, if any, and closes the field.
    pub fn commit_rename(&mut self) {
        let Some(id) = self.renaming.session.take() else { return };
        let typed: String = self.renaming.edit.text.trim().chars().take(MAX_TITLE).collect();
        self.renaming.edit = TextEdit::new("");
        if let Some(session) = self.session_mut(id)
            && !typed.is_empty()
            && session.title != typed
        {
            session.title = typed;
            self.persist(id);
            self.sessions_changed();
        }
        self.focus_composer(self.selected);
    }

    /// Closes the field and keeps the old name.
    pub fn cancel_rename(&mut self) {
        if self.renaming.session.take().is_some() {
            self.renaming.edit = TextEdit::new("");
            self.focus_composer(self.selected);
        }
    }

    /// The field for `id`'s name, while it is being renamed.
    pub fn renaming_field(&self, id: SessionId) -> Option<&TextEdit> {
        (self.renaming.session == Some(id)).then_some(&self.renaming.edit)
    }
}

/// The `rename` scene: the second session's row turned into a field, a new name typed in it.
pub(crate) fn scene(s: &mut KrampstaApp) {
    let Some(id) = s.sessions.get(1).map(|x| x.id) else { return };
    s.start_rename(id);
    s.renaming.edit = TextEdit::new("Retry backoff, take two");
    s.renaming.edit.focused = true;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::{MemoryStore, MockCatalog, MockConfig, SessionStore};

    use super::*;

    fn app() -> KrampstaApp {
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant()))).with_sample_sessions()
    }

    #[test]
    fn a_typed_name_is_kept_and_a_blank_or_cancelled_one_is_not() {
        let mut app = app();
        let id = app.sessions[0].id;
        let old = app.sessions[0].title.clone();

        app.start_rename(id);
        assert_eq!(app.renaming_field(id).map(|e| e.text.as_str()), Some(old.as_str()), "it starts from the title");
        app.renaming.edit = TextEdit::new("   ");
        app.commit_rename();
        assert_eq!(app.session(id).unwrap().title, old, "blank: the old name stays");

        app.start_rename(id);
        app.renaming.edit = TextEdit::new("Something else");
        app.cancel_rename();
        assert_eq!(app.session(id).unwrap().title, old, "Escape: the old name stays");
        assert!(app.renaming_field(id).is_none());

        app.start_rename(id);
        app.renaming.edit = TextEdit::new("  Retry backoff  ");
        app.commit_rename();
        assert_eq!(app.session(id).unwrap().title, "Retry backoff", "trimmed");
        assert!(app.draft_of(app.selected).focused, "the caret goes back to the composer");
    }

    #[test]
    fn a_rename_left_open_is_kept_when_another_session_is_chosen() {
        let mut app = app();
        let (first, second) = (app.sessions[0].id, app.sessions[1].id);
        app.start_rename(first);
        app.renaming.edit = TextEdit::new("Renamed");
        app.select(second);
        assert_eq!(app.session(first).unwrap().title, "Renamed");
        assert!(app.renaming_field(first).is_none());
    }

    #[test]
    fn the_new_name_is_saved() {
        let store = Arc::new(MemoryStore::new());
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())))
            .with_sample_sessions()
            .with_store(store.clone() as Arc<dyn SessionStore>);
        let id = app.sessions[0].id;
        app.start_rename(id);
        app.renaming.edit = TextEdit::new("Saved name");
        app.commit_rename();
        assert!(store.sessions().iter().any(|s| s.id == id && s.title == "Saved name"), "{:?}", store.sessions());
    }
}
