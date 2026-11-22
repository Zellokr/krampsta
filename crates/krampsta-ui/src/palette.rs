//! The command palette (Ctrl+K): a field that finds a command or a session by name, and runs
//! or opens it. Up and Down move, Return runs, Escape closes. The list (at most [`MAX_SHOWN`]
//! rows) is worked out when the palette opens and as the query changes, never per frame; the
//! view (`views/palette.rs`) only draws it.

use krampsta_core::SessionId;
use rune::{TextEdit, Tx};

use crate::files::Inspector;
use crate::state::{KrampstaApp, Screen};
use crate::theme::ThemeChoice;

/// The most rows the palette shows: the best matches.
pub const MAX_SHOWN: usize = 12;

/// What a row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewSession,
    OpenFolder,
    SearchSessions,
    ToggleSidebar,
    TogglePanel,
    Show(Inspector),
    Split,
    CloseTab,
    Settings,
    Theme(ThemeChoice),
    Session(SessionId),
}

/// One row: what it does, its name, and what it shows on the right (a shortcut, a project).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub action: Action,
    pub label: String,
    pub detail: String,
}

/// The palette's state.
#[derive(Default)]
pub struct Palette {
    pub open: bool,
    pub query: TextEdit,
    /// The rows shown, best first.
    pub entries: Vec<Entry>,
    /// The row Return runs.
    pub active: usize,
}

/// `win` on Windows and Linux, `mac` on macOS.
fn keys(win: &str, mac: &str) -> String {
    if cfg!(target_os = "macos") { mac } else { win }.to_owned()
}

impl KrampstaApp {
    /// Opens the palette (or closes it, if open), with the caret in its field.
    pub fn toggle_palette(&mut self) {
        if self.palette.open {
            self.close_palette();
            return;
        }
        self.menu = None;
        for view in self.views.values_mut() {
            view.draft.focused = false;
        }
        self.palette = Palette { open: true, ..Palette::default() };
        self.palette.query.focused = true;
        self.refresh_palette();
    }

    pub fn close_palette(&mut self) {
        if !self.palette.open {
            return;
        }
        self.palette = Palette::default();
        self.focus_composer(self.selected);
    }

    /// Works the rows out again for the query as it stands.
    pub fn refresh_palette(&mut self) {
        let query = self.palette.query.text.trim().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut found: Vec<(u8, Entry)> = self
            .palette_candidates()
            .into_iter()
            .filter_map(|entry| {
                let haystack = format!("{} {}", entry.label, entry.detail).to_lowercase();
                if !words.iter().all(|w| haystack.contains(w)) {
                    return None;
                }
                // A name that starts with the query first, then one with a word that does.
                let label = entry.label.to_lowercase();
                let rank = if query.is_empty() || label.starts_with(&query) {
                    0
                } else if label.split_whitespace().any(|w| w.starts_with(words[0])) {
                    1
                } else {
                    2
                };
                Some((rank, entry))
            })
            .collect();
        // Stable: commands keep their order, then sessions newest first.
        found.sort_by_key(|(rank, _)| *rank);
        self.palette.entries = found.into_iter().map(|(_, e)| e).take(MAX_SHOWN).collect();
        self.palette.active = 0;
    }

    /// Every command, then every session.
    fn palette_candidates(&self) -> Vec<Entry> {
        let entry = |action, label: &str, detail: String| Entry { action, label: label.to_owned(), detail };
        let mut all = vec![
            entry(Action::NewSession, "New session", keys("Ctrl+N", "⌘N")),
            entry(Action::OpenFolder, "Open folder…", keys("Ctrl+O", "⌘O")),
            entry(Action::SearchSessions, "Search sessions", keys("Ctrl+Shift+F", "⇧⌘F")),
            entry(Action::ToggleSidebar, "Show or hide the sidebar", keys("Ctrl+Shift+B", "⇧⌘B")),
            entry(Action::TogglePanel, "Show or hide the right panel", keys("Ctrl+J", "⌘J")),
            entry(Action::Show(Inspector::Changes), "Show the proposed changes", String::new()),
            entry(Action::Show(Inspector::Git), "Show what differs from the last commit (Git)", String::new()),
            entry(Action::Show(Inspector::Files), "Show the project's files", String::new()),
            entry(Action::Split, "Split the conversation", keys("Ctrl+\\", "⌘\\")),
            entry(Action::CloseTab, "Close the tab", keys("Ctrl+W", "⌘W")),
            entry(Action::Settings, "Open settings", keys("Ctrl+,", "⌘,")),
            entry(Action::Theme(ThemeChoice::Dark), "Theme: dark", String::new()),
            entry(Action::Theme(ThemeChoice::Light), "Theme: light", String::new()),
            entry(Action::Theme(ThemeChoice::System), "Theme: as the system", String::new()),
        ];
        // A session not tied to a folder belongs to the open one, as the sidebar groups it.
        let open = self.project.as_ref().map(|p| p.root.as_path());
        all.extend(self.sessions.iter().map(|s| Entry {
            action: Action::Session(s.id),
            label: s.title.clone(),
            detail: s.project.as_deref().or(open).map_or_else(|| "Session".to_owned(), krampsta_agent::project_name),
        }));
        all
    }

    /// Moves the active row by `by`, wrapping around.
    pub fn palette_move(&mut self, by: isize) {
        let n = self.palette.entries.len();
        if n == 0 {
            return;
        }
        self.palette.active = (self.palette.active as isize + by).rem_euclid(n as isize) as usize;
    }

    /// Runs row `index` and closes the palette.
    pub fn run_palette(&mut self, index: usize, tx: &mut Tx) {
        let Some(action) = self.palette.entries.get(index).map(|e| e.action) else { return };
        self.close_palette();
        match action {
            Action::NewSession => {
                self.new_session();
            }
            Action::OpenFolder => self.choose_project(tx),
            Action::SearchSessions => {
                if !self.sidebar.is_open() {
                    self.sidebar.toggle();
                }
                self.focus_search();
            }
            Action::ToggleSidebar => self.sidebar.toggle(),
            Action::TogglePanel => self.toggle_changes(),
            Action::Show(inspector) => self.show_inspector(inspector),
            Action::Split => {
                self.screen = Screen::Chat;
                self.toggle_split();
            }
            Action::CloseTab => self.close_tab(self.selected),
            Action::Settings => self.open_settings(),
            Action::Theme(theme) => self.theme = theme,
            Action::Session(id) => self.select(id),
        }
    }
}

/// The `palette` scene: the palette open, "s" typed: commands and a session that match.
pub(crate) fn scene(s: &mut KrampstaApp) {
    s.toggle_palette();
    s.palette.query = TextEdit::new("s");
    s.palette.query.focused = true;
    s.refresh_palette();
    s.palette.active = 1;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::{MockCatalog, MockConfig};

    use super::*;

    fn app() -> KrampstaApp {
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant()))).with_sample_sessions()
    }

    fn query(app: &mut KrampstaApp, text: &str) {
        app.palette.query = TextEdit::new(text);
        app.refresh_palette();
    }

    fn labels(app: &KrampstaApp) -> Vec<&str> {
        app.palette.entries.iter().map(|e| e.label.as_str()).collect()
    }

    #[test]
    fn it_opens_with_the_commands_and_finds_by_any_words() {
        let mut app = app();
        app.toggle_palette();
        assert!(app.palette.open && app.palette.query.focused);
        assert_eq!(app.palette.entries.len(), MAX_SHOWN, "the first rows, capped");
        assert_eq!(app.palette.entries[0].label, "New session");

        query(&mut app, "theme li");
        assert_eq!(labels(&app), ["Theme: light"]);
        query(&mut app, "git");
        assert_eq!(app.palette.entries[0].action, Action::Show(Inspector::Git));
        query(&mut app, "jitter");
        assert!(matches!(app.palette.entries[0].action, Action::Session(_)), "sessions by title");
        query(&mut app, "zzz nothing");
        assert!(app.palette.entries.is_empty());
    }

    #[test]
    fn a_name_that_starts_with_the_query_comes_first() {
        let mut app = app();
        app.toggle_palette();
        query(&mut app, "s");
        // "Search sessions", "Split…", "Show…" start with it; "New session" only holds it.
        let first = &app.palette.entries[0].label;
        assert!(first.to_lowercase().starts_with('s'), "{first}");
    }

    #[test]
    fn up_and_down_wrap_and_return_runs_the_row() {
        let mut app = app();
        app.toggle_palette();
        query(&mut app, "theme");
        assert_eq!(app.palette.entries.len(), 3);
        app.palette_move(-1);
        assert_eq!(app.palette.active, 2, "up from the first wraps to the last");
        app.palette_move(1);
        assert_eq!(app.palette.active, 0);
        app.palette_move(1);
        // A runtime hands the row its `Tx`, as a press or Return does.
        let mut rt = rune::Runtime::new(app, crate::app::view, (1200.0, 800.0), rune::Env::new(true, 1.0));
        rt.update(|s, tx| {
            let active = s.palette.active;
            s.run_palette(active, tx);
        });
        assert_eq!(rt.state.theme, ThemeChoice::Light);
        assert!(!rt.state.palette.open, "running closes it");
        assert!(rt.state.draft_of(rt.state.selected).focused, "the caret goes back to the composer");
    }
}
