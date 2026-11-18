//! The UI's state and the operations its handlers perform.
//!
//! Everything a view reads lives in [`KrampstaApp`]. Handlers call its methods; agent events
//! reach it through [`KrampstaApp::pump`], which `poll` runs when the agent host wakes the window.

use std::collections::HashSet;
use std::sync::Arc;

use krampsta_agent::{AgentError, AgentHost, EngineCatalog, MockAgent, MockCatalog, MockConfig, Notifier, RunRequest};
use krampsta_core::{AgentEvent, IdGen, RunId, Session, SessionError, SessionId, ToolCallId};
use rune::TextEdit;
use rune_kit::PaneState;
use rune_kit::diff::DiffState;

use crate::accounts::SignIn;
use crate::picker::EngineChoice;
use crate::sample;
use crate::theme::ThemeChoice;

/// Which full-window screen shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Chat,
    Settings,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("could not start the agent: {0}")]
    Agent(#[from] AgentError),
    /// The chosen provider and model can't run (no model yet, missing key…).
    #[error("{0}")]
    Engine(String),
}

pub struct KrampstaApp {
    /// Newest first, as the sidebar lists them. Never empty.
    pub sessions: Vec<Session>,
    pub selected: SessionId,
    /// The composer's text.
    pub draft: TextEdit,
    /// A one-line message under the composer (why a send didn't go through).
    pub notice: Option<String>,
    pub screen: Screen,
    pub theme: ThemeChoice,
    pub sidebar: PaneState,
    /// The right-hand changes panel.
    pub changes: PaneState,
    pub diff: DiffState,
    /// The provider and model pickers.
    pub engine: EngineChoice,
    /// The account sign-in dialog.
    pub sign_in: SignIn,
    /// Tool cards the user opened in the transcript.
    pub open_tools: HashSet<ToolCallId>,
    pub(crate) catalog: Arc<dyn EngineCatalog>,
    pub(crate) agent: AgentHost,
    ids: IdGen,
    window_width: Option<f32>,
    auto_hidden: AutoHidden,
    keep: Pane,
}

impl KrampstaApp {
    /// A fresh app running what `catalog` offers, starting from its initial selection, with
    /// the sample sessions.
    pub fn new(catalog: Arc<dyn EngineCatalog>) -> Self {
        let ids = IdGen::new();
        let sessions = sample::sessions(&ids);
        let selected = sessions.first().map_or(SessionId(0), |s| s.id);
        let mut draft = TextEdit::new("");
        draft.focused = true;
        let engine = EngineChoice::new(catalog.providers(), catalog.initial());
        // A stand-in until `rebuild_engine` below. With no usable selection sends are refused,
        // so it never runs.
        let agent = AgentHost::new(Arc::new(MockAgent::new(MockConfig::default())));
        let mut app = KrampstaApp {
            sessions,
            selected,
            draft,
            notice: None,
            screen: Screen::Chat,
            theme: ThemeChoice::Dark,
            sidebar: PaneState::new(264.0).range(200.0, 420.0),
            changes: PaneState::new(440.0).range(320.0, 760.0),
            diff: sample::diff(),
            engine,
            sign_in: SignIn::Closed,
            open_tools: HashSet::new(),
            catalog,
            agent,
            ids,
            window_width: None,
            auto_hidden: AutoHidden::default(),
            keep: Pane::Sidebar,
        };
        app.keep_selection_usable();
        app.rebuild_engine();
        app
    }

    /// The default app: the mock agent at its normal pace. `AppSpec::init` needs a plain `fn`.
    pub fn with_mock() -> Self {
        Self::new(Arc::new(MockCatalog::new(MockConfig::default())))
    }

    /// Wakes the window when agent events arrive.
    pub fn set_notifier(&mut self, notify: Notifier) {
        self.agent.set_notifier(notify);
    }

    pub fn engine_name(&self) -> &str {
        self.agent.engine_name()
    }

    pub fn session(&self, id: SessionId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    fn session_mut(&mut self, id: SessionId) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    /// The selected session. Falls back to the first if the selection went stale.
    pub fn current(&self) -> Option<&Session> {
        self.session(self.selected).or_else(|| self.sessions.first())
    }

    /// Adds an empty session at the top and selects it.
    /// Opens a closed tool card, closes an open one.
    pub fn toggle_tool(&mut self, id: &ToolCallId) {
        if !self.open_tools.remove(id) {
            self.open_tools.insert(id.clone());
        }
    }

    pub fn new_session(&mut self) -> SessionId {
        let id = SessionId(self.ids.next());
        self.sessions.insert(0, Session::new(id));
        self.select(id);
        id
    }

    pub fn select(&mut self, id: SessionId) {
        if self.session(id).is_some() {
            self.selected = id;
            self.screen = Screen::Chat;
            self.notice = None;
            self.draft.focused = true;
        }
    }

    /// Sends the composer's text to the agent in the selected session.
    pub fn send(&mut self) -> Result<RunId, SendError> {
        if let Some(problem) = &self.engine.problem {
            return Err(SendError::Engine(problem.clone()));
        }
        let run = self.agent.next_run_id();
        let text = self.draft.text.clone();
        let id = self.current().map(|s| s.id).ok_or(SessionError::EmptyMessage)?;
        let session = self.session_mut(id).ok_or(SessionError::EmptyMessage)?;
        session.submit(&text, run)?;
        let request = RunRequest { session: id, run, history: session.messages.clone() };
        self.draft = TextEdit::new("");
        self.draft.focused = true;
        self.notice = None;
        if let Err(e) = self.agent.start(request) {
            if let Some(s) = self.session_mut(id) {
                s.apply(&AgentEvent::Failed { run, error: e.to_string() });
            }
            return Err(e.into());
        }
        Ok(run)
    }

    /// `send` for a handler: problems become the composer's notice instead of an error.
    pub fn submit(&mut self) {
        match self.send() {
            Ok(_) | Err(SendError::Session(SessionError::EmptyMessage)) => {}
            Err(e) => self.notice = Some(e.to_string()),
        }
    }

    /// Stops the selected session's run. Returns whether one was running.
    pub fn cancel(&mut self) -> bool {
        match self.current().and_then(Session::active_run) {
            Some(run) => self.agent.cancel(run),
            None => false,
        }
    }

    /// Applies every agent event that has arrived. True when anything visible changed.
    pub fn pump(&mut self) -> bool {
        let accounts = self.agent.drain_accounts();
        let lists = self.agent.drain_models();
        let mut changed = self.land_accounts(accounts);
        changed |= self.land_models(lists);
        for e in self.agent.drain() {
            if let Some(s) = self.session_mut(e.session) {
                changed |= s.apply(&e.event);
            }
        }
        changed
    }

    /// Shows or hides the changes panel, then refits the window (the sidebar may make room).
    pub fn toggle_changes(&mut self) {
        self.changes.toggle();
        self.auto_hidden.changes = false;
        self.keep = if self.changes.is_open() { Pane::Changes } else { Pane::Sidebar };
        if let Some(w) = self.window_width {
            self.fit_width(w);
        }
    }

    /// Keeps the conversation readable as the window narrows: side panes close (the one the
    /// user opened last closes last) and reopen once there is room again. Runs whenever the
    /// window's width changes.
    pub fn fit_width(&mut self, width: f32) {
        self.window_width = Some(width);
        let needed_for = |main: f32, side: Option<f32>, changes: Option<f32>| {
            main + PANE_SLACK + side.unwrap_or(0.0) + changes.unwrap_or(0.0)
        };
        let needed = |side: Option<f32>, changes: Option<f32>| needed_for(MIN_MAIN, side, changes);
        let shown = |p: &PaneState| p.is_open().then(|| p.width());

        // Reopen what we hid, when it fits with a little hysteresis.
        if self.auto_hidden.sidebar && needed(Some(self.sidebar.width()), shown(&self.changes)) + 40.0 <= width {
            self.sidebar.set_open(true);
            self.auto_hidden.sidebar = false;
        }
        if self.auto_hidden.changes && needed(shown(&self.sidebar), Some(self.changes.width())) + 40.0 <= width {
            self.changes.set_open(true);
            self.auto_hidden.changes = false;
        }

        let order = match self.keep {
            Pane::Changes => [Pane::Sidebar, Pane::Changes],
            Pane::Sidebar => [Pane::Changes, Pane::Sidebar],
        };
        for pane in order {
            // The pane the user opened last only gives way below a harder floor.
            let main = if pane == self.keep { MIN_MAIN_HARD } else { MIN_MAIN };
            if needed_for(main, shown(&self.sidebar), shown(&self.changes)) <= width {
                continue;
            }
            let (state, flag) = match pane {
                Pane::Sidebar => (&mut self.sidebar, &mut self.auto_hidden.sidebar),
                Pane::Changes => (&mut self.changes, &mut self.auto_hidden.changes),
            };
            if state.is_open() {
                state.set_open(false);
                *flag = true;
            }
        }
    }
}

/// The narrowest the conversation column may get before side panes close.
pub const MIN_MAIN: f32 = 440.0;
/// The floor for the pane the user opened most recently.
pub const MIN_MAIN_HARD: f32 = 320.0;
/// Frame insets and pane padding around the panes' own widths.
const PANE_SLACK: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Sidebar,
    Changes,
}

/// Panes [`KrampstaApp::fit_width`] closed for lack of room (and will reopen).
#[derive(Clone, Copy, Debug, Default)]
struct AutoHidden {
    sidebar: bool,
    changes: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use krampsta_core::{AgentStatus, MessageRole};
    use std::time::{Duration, Instant};

    fn app() -> KrampstaApp {
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())))
    }

    fn pump_until_idle(app: &mut KrampstaApp) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.current().is_some_and(|s| s.status.is_running()) {
            assert!(Instant::now() < deadline, "the run never finished");
            app.pump();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn starts_with_a_selected_sample_session() {
        let app = app();
        assert!(!app.sessions.is_empty());
        assert_eq!(app.current().map(|s| s.id), Some(app.selected));
    }

    #[test]
    fn new_sessions_go_on_top_and_are_selected() {
        let mut app = app();
        let before = app.sessions.len();
        let id = app.new_session();
        assert_eq!(app.sessions.len(), before + 1);
        assert_eq!(app.sessions[0].id, id);
        assert_eq!(app.selected, id);
        assert_eq!(app.current().map(|s| s.messages.len()), Some(0));
    }

    #[test]
    fn selecting_an_unknown_session_keeps_the_selection() {
        let mut app = app();
        let selected = app.selected;
        app.select(SessionId(9_999));
        assert_eq!(app.selected, selected);
    }

    #[test]
    fn sending_runs_the_agent_and_its_reply_lands() {
        let mut app = app();
        app.new_session();
        app.draft = TextEdit::new("Refactor the parser");
        app.send().unwrap();
        assert!(app.draft.text.is_empty(), "the composer clears");
        assert_eq!(app.current().map(|s| s.status.clone()), Some(AgentStatus::Running));
        pump_until_idle(&mut app);
        let s = app.current().unwrap();
        assert_eq!(s.status, AgentStatus::Completed);
        assert_eq!(s.title, "Refactor the parser");
        assert_eq!(s.messages.len(), 2);
        assert_eq!(s.messages[1].role, MessageRole::Assistant);
        assert!(s.messages[1].content.contains("Refactor the parser"));
    }

    #[test]
    fn blank_drafts_are_not_sent() {
        let mut app = app();
        app.draft = TextEdit::new("   ");
        assert!(matches!(app.send(), Err(SendError::Session(SessionError::EmptyMessage))));
        app.submit();
        assert_eq!(app.notice, None, "an empty send is silently ignored");
    }

    #[test]
    fn fail_command_shows_the_failed_state() {
        let mut app = app();
        app.draft = TextEdit::new("/fail now");
        app.send().unwrap();
        pump_until_idle(&mut app);
        assert!(matches!(app.current().map(|s| &s.status), Some(AgentStatus::Failed { .. })));
    }

    #[test]
    fn narrow_windows_close_side_panes_and_wide_ones_reopen_them() {
        let mut app = app();
        app.fit_width(1200.0);
        assert!(app.sidebar.is_open() && app.changes.is_open(), "everything fits at 1200");
        app.fit_width(900.0);
        assert!(app.sidebar.is_open() && !app.changes.is_open(), "changes go first");
        app.fit_width(600.0);
        assert!(!app.sidebar.is_open(), "then the sidebar");
        app.fit_width(1300.0);
        assert!(app.sidebar.is_open() && app.changes.is_open(), "both come back");
    }

    #[test]
    fn opening_changes_in_a_narrow_window_makes_room_from_the_sidebar() {
        let mut app = app();
        app.fit_width(900.0);
        assert!(!app.changes.is_open());
        app.toggle_changes();
        assert!(app.changes.is_open(), "the user's choice wins");
        assert!(!app.sidebar.is_open(), "the sidebar makes room");
    }

    #[test]
    fn cancel_stops_a_slow_run() {
        let slow = MockConfig { think: Duration::from_secs(30), ..MockConfig::default() };
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::new(slow)));
        app.draft = TextEdit::new("take your time");
        app.send().unwrap();
        assert!(app.cancel());
        pump_until_idle(&mut app);
        assert_eq!(app.current().map(|s| s.status.clone()), Some(AgentStatus::Cancelled));
        assert!(!app.cancel(), "nothing left to cancel");
    }
}
