//! Searching the sessions from the sidebar: a field above the list, matching each session's
//! title and the text of its messages, ignoring case. The list (and its project groups) shows
//! only the matches, unfolded. Ctrl+Shift+F puts the caret in the field; Escape or its clear
//! button empties it.
//!
//! The matches are worked out when the query changes and at a few moments (the history lands,
//! a run ends, a session is renamed), never per frame: the view only asks [`KrampstaApp::shows`].

use std::collections::HashSet;

use krampsta_core::{Session, SessionId};
use rune::TextEdit;

use crate::state::KrampstaApp;

/// The query and the sessions it matches.
#[derive(Default)]
pub struct SessionSearch {
    pub query: TextEdit,
    /// The sessions that match; `None` while there is no query (every session shows).
    matches: Option<HashSet<SessionId>>,
}

impl SessionSearch {
    /// Whether a query is filtering the list.
    pub fn is_active(&self) -> bool {
        self.matches.is_some()
    }
}

impl KrampstaApp {
    /// Whether the sidebar lists `id`.
    pub fn shows(&self, id: SessionId) -> bool {
        self.search.matches.as_ref().is_none_or(|m| m.contains(&id))
    }

    /// How many sessions the query matches; `None` without a query.
    pub fn search_count(&self) -> Option<usize> {
        self.search.matches.as_ref().map(HashSet::len)
    }

    /// Works the matches out again for the query as it stands.
    pub fn refresh_search(&mut self) {
        let query = self.search.query.text.trim().to_lowercase();
        self.search.matches =
            (!query.is_empty()).then(|| self.sessions.iter().filter(|s| matches(s, &query)).map(|s| s.id).collect());
    }

    /// Empties the field: every session shows again.
    pub fn clear_search(&mut self) {
        let focused = self.search.query.focused;
        self.search.query = TextEdit::new("");
        self.search.query.focused = focused;
        self.search.matches = None;
    }

    /// Puts the caret in the search field.
    pub fn focus_search(&mut self) {
        for view in self.views.values_mut() {
            view.draft.focused = false;
        }
        self.search.query.focused = true;
    }

    /// The sessions changed (history landed, a run ended, a rename): match them again, if
    /// searching.
    pub(crate) fn sessions_changed(&mut self) {
        if self.search.is_active() {
            self.refresh_search();
        }
    }
}

/// Whether `session`'s title or any of its messages holds `query` (lowercase).
fn matches(session: &Session, query: &str) -> bool {
    session.title.to_lowercase().contains(query)
        || session.messages.iter().any(|m| m.content.to_lowercase().contains(query))
}

/// The `search` scene: the sidebar searching for a word only one session's messages hold.
pub(crate) fn scene(s: &mut KrampstaApp) {
    s.search.query = TextEdit::new("jitter");
    s.search.query.focused = true;
    for view in s.views.values_mut() {
        view.draft.focused = false;
    }
    s.refresh_search();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::{MockCatalog, MockConfig};
    use krampsta_core::MessageRole;

    use super::*;

    fn app() -> KrampstaApp {
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())));
        app.sessions.clear();
        let mut parser = Session::with_title(SessionId(1), "Where is the parser?");
        parser.push_message(MessageRole::Assistant, "It lives in `src/parse.rs`.");
        let mut retry = Session::with_title(SessionId(2), "Retry backoff");
        retry.push_message(MessageRole::User, "Add JITTER to the waits");
        app.sessions = vec![parser, retry];
        app.selected = SessionId(1);
        app
    }

    fn search(app: &mut KrampstaApp, query: &str) {
        app.search.query = TextEdit::new(query);
        app.refresh_search();
    }

    #[test]
    fn titles_and_messages_match_ignoring_case() {
        let mut app = app();
        assert!(app.shows(SessionId(1)) && app.shows(SessionId(2)), "no query: everything shows");
        search(&mut app, "PARSER");
        assert!(app.shows(SessionId(1)) && !app.shows(SessionId(2)), "a title");
        search(&mut app, "jitter");
        assert!(!app.shows(SessionId(1)) && app.shows(SessionId(2)), "a message");
        search(&mut app, "parse.rs");
        assert_eq!(app.search_count(), Some(1));
        search(&mut app, "nothing like this");
        assert_eq!(app.search_count(), Some(0));
        search(&mut app, "   ");
        assert_eq!(app.search_count(), None, "blank: no filter");
    }

    #[test]
    fn the_groups_hold_only_the_matches_and_clearing_shows_all() {
        let mut app = app();
        search(&mut app, "retry");
        let listed: Vec<SessionId> = app.groups().iter().flat_map(|g| g.sessions.iter().map(|s| s.id)).collect();
        assert_eq!(listed, [SessionId(2)]);
        app.clear_search();
        assert_eq!(app.groups().iter().map(|g| g.sessions.len()).sum::<usize>(), 2);
    }

    #[test]
    fn a_rename_is_matched_at_once() {
        let mut app = app();
        search(&mut app, "favourite");
        assert_eq!(app.search_count(), Some(0));
        app.start_rename(SessionId(2));
        app.renaming.edit = TextEdit::new("My favourite session");
        app.commit_rename();
        assert!(app.shows(SessionId(2)), "renamed into the query: it shows");
    }
}
