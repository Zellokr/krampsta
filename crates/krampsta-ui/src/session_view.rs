//! What the window keeps for each session it shows, apart from the conversation itself: the
//! composer's unsent text and the reply being paced out. Kept per session, a draft survives
//! switching to another session and back, and sessions shown side by side stream on their own.

use krampsta_core::SessionId;
use rune::TextEdit;

use crate::pace::Pace;
use crate::state::KrampstaApp;

/// One session's view state.
#[derive(Debug)]
pub struct SessionView {
    /// The composer's text.
    pub draft: TextEdit,
    /// Its streaming reply, while it comes out at a steady pace.
    pub(crate) pace: Option<Pace>,
}

impl Default for SessionView {
    fn default() -> Self {
        SessionView { draft: TextEdit::new(""), pace: None }
    }
}

impl KrampstaApp {
    /// The composer's text in the selected session.
    pub fn draft(&self) -> &TextEdit {
        self.draft_of(self.selected)
    }

    /// The composer's text in `session` (empty until typed into).
    pub fn draft_of(&self, session: SessionId) -> &TextEdit {
        self.views.get(&session).map_or(&self.blank_draft, |v| &v.draft)
    }

    /// The composer's text in the selected session, to edit.
    pub fn draft_mut(&mut self) -> &mut TextEdit {
        let selected = self.selected;
        &mut self.view_mut(selected).draft
    }

    /// Replaces the selected session's composer text (tests, scenes).
    pub fn set_draft(&mut self, text: &str) {
        let draft = self.draft_mut();
        *draft = TextEdit::new(text);
        draft.focused = true;
    }

    pub(crate) fn view_mut(&mut self, session: SessionId) -> &mut SessionView {
        self.views.entry(session).or_default()
    }

    pub(crate) fn pace_of(&self, session: SessionId) -> Option<Pace> {
        self.views.get(&session).and_then(|v| v.pace)
    }

    /// The sessions on screen now.
    pub(crate) fn visible_sessions(&self) -> Vec<SessionId> {
        match self.screen {
            crate::state::Screen::Chat => vec![self.selected],
            crate::state::Screen::Settings => Vec::new(),
        }
    }

    /// Puts the caret in `session`'s composer and takes it out of every other.
    pub(crate) fn focus_composer(&mut self, session: SessionId) {
        for (id, view) in &mut self.views {
            view.draft.focused = *id == session;
        }
        self.view_mut(session).draft.focused = true;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::{MockCatalog, MockConfig};

    use super::*;

    #[test]
    fn a_draft_stays_with_its_session() {
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())));
        let first = app.selected;
        app.set_draft("half a thought");
        let second = app.new_session();
        assert_eq!(app.draft().text, "", "a new session starts blank");
        app.set_draft("another");
        app.select(first);
        assert_eq!(app.draft().text, "half a thought");
        assert!(app.draft().focused && !app.draft_of(second).focused, "the caret follows the selection");
    }
}
