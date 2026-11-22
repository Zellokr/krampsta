//! The UI's state and the operations its handlers perform.
//!
//! Everything a view reads lives in [`KrampstaApp`]. Handlers call its methods; agent events
//! reach it through [`KrampstaApp::pump`], which `poll` runs when the agent host wakes the window.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use krampsta_agent::{
    AgentError, AgentHost, EngineCatalog, History, MockAgent, MockCatalog, MockConfig, Notifier, ProjectOpener,
    RunRequest, Selection, SessionStore, Toolbox,
};
use krampsta_core::{AgentEvent, IdGen, MessageId, RunId, Session, SessionError, SessionId, ToolCallId};
use rune::TextEdit;
use rune_kit::PaneState;
use rune_kit::diff::{DiffState, FileDiff};

use crate::accounts::{KeyField, SignIn};
use crate::approvals::Asking;
use crate::changes::Changes;
use crate::costs::{Price, PriceField};
use crate::menus::OpenMenu;
use crate::panes::Split;
use crate::picker::EngineChoice;
use crate::prefs::PrefsSync;
use crate::projects::ProjectInfo;
use crate::sample;
use crate::session_view::SessionView;
use crate::theme::ThemeChoice;
use rune_kit::shell::TabDragState;

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
    /// The session works in a folder other than the one open (the tools are that one's).
    #[error("This session is from {0}. Open that folder to continue it.")]
    OtherProject(String),
}

pub struct KrampstaApp {
    /// Newest first, as the sidebar lists them. Never empty.
    pub sessions: Vec<Session>,
    pub selected: SessionId,
    /// Sessions open in tabs, in order; the selected one is the active tab. Never empty.
    pub tabs: Vec<SessionId>,
    pub(crate) tab_drag: TabDragState,
    /// A second session shown beside the selected one, if the conversation is split.
    pub split: Option<Split>,
    /// What the window keeps per session: the unsent draft, the reply being paced out.
    pub(crate) views: HashMap<SessionId, SessionView>,
    /// What a session with nothing typed yet shows in its composer.
    pub(crate) blank_draft: TextEdit,
    /// A one-line message under the composer (why a send didn't go through).
    pub notice: Option<String>,
    pub screen: Screen,
    pub theme: ThemeChoice,
    pub sidebar: PaneState,
    /// The right-hand changes panel.
    pub changes: PaneState,
    pub diff: DiffState,
    /// The changes the agent proposed, under review in the changes panel.
    pub review: Changes,
    /// Commands waiting for the user to allow them, the first one in the dialog.
    pub approvals: VecDeque<Asking>,
    /// The provider and model pickers.
    pub engine: EngineChoice,
    /// The account sign-in dialog.
    pub sign_in: SignIn,
    /// The API key field in settings.
    pub api_key: KeyField,
    /// The change each `edit_file` call proposed, as its card shows it.
    pub card_diffs: HashMap<ToolCallId, FileDiff>,
    /// Tool cards the user opened in the transcript.
    pub open_tools: HashSet<ToolCallId>,
    /// Commands announced while the changes panel held unapplied proposals.
    pub(crate) commands_with_pending_changes: HashSet<(RunId, ToolCallId)>,
    /// Messages whose reasoning the user opened.
    pub open_reasoning: HashSet<MessageId>,
    /// The context menu showing, if any.
    pub menu: Option<OpenMenu>,
    /// The session the delete confirmation asks about.
    pub confirm_delete: Option<SessionId>,
    pub(crate) catalog: Arc<dyn EngineCatalog>,
    pub(crate) agent: AgentHost,
    /// Where sessions are saved. None: nothing outlives the window (tests, headless scenes).
    pub(crate) store: Option<Arc<dyn SessionStore>>,
    /// The folder the agent's tools work in. None: runs have no tools.
    pub project: Option<ProjectInfo>,
    /// Projects whose sessions the user folded away in the sidebar (None: "No project").
    pub collapsed_groups: HashSet<Option<PathBuf>>,
    /// The folder being opened in the background, if one is.
    pub opening_project: Option<PathBuf>,
    /// What each billed model costs, as the user set it (provider, model).
    pub prices: BTreeMap<(String, String), Price>,
    /// The price fields the settings show, prepared as they open.
    pub price_fields: Vec<PriceField>,
    /// What a price field edits once it no longer exists (see `price_text`).
    pub(crate) price_scratch: TextEdit,
    /// The settings saved last, to tell when they change.
    pub(crate) prefs: PrefsSync,
    /// Turns a folder into tools. None: folders can't be opened (tests, scenes).
    pub(crate) opener: Option<Arc<dyn ProjectOpener>>,
    /// What the right panel shows: the changes, or the project's files.
    pub inspector: crate::files::Inspector,
    /// What differs from the last commit, as the "Git" view shows it.
    pub repo: crate::git::RepoView,
    /// The project's files, as the "Files" view shows them.
    pub files: crate::files::ProjectTree,
    /// The MCP servers, as Settings shows them.
    pub servers: crate::servers::ServerList,
    /// Reads pasted and saved images off the UI thread.
    pub(crate) images: crate::images::Images,
    ids: IdGen,
    window_width: Option<f32>,
    auto_hidden: AutoHidden,
    keep: Pane,
}

impl KrampstaApp {
    /// Lets runs started from now on call `tools` (the project folder's).
    pub fn with_tools(mut self, tools: Arc<dyn Toolbox>) -> Self {
        self.agent.set_toolbox(Some(tools));
        self
    }

    /// Replaces the sessions with the example ones, the first selected (stills and tests: the
    /// real app starts from the user's own history).
    pub fn with_sample_sessions(mut self) -> Self {
        self.sessions = sample::sessions(&self.ids);
        if let Some(first) = self.sessions.first() {
            self.selected = first.id;
        }
        self.views.clear();
        self.tabs = vec![self.selected];
        self.focus_composer(self.selected);
        self
    }

    /// Saves sessions to `store` from now on: each one once it has a message, again as its
    /// runs make progress, and removes it when deleted. Session ids then come from the clock,
    /// so they never repeat a saved one; the sessions already here are renumbered.
    pub fn with_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.ids = IdGen::from_clock();
        let mut views = std::mem::take(&mut self.views);
        for session in &mut self.sessions {
            let id = SessionId(self.ids.next());
            if self.selected == session.id {
                self.selected = id;
            }
            // Its draft, focus and tab follow it to the new id.
            if let Some(view) = views.remove(&session.id) {
                self.views.insert(id, view);
            }
            for tab in &mut self.tabs {
                if *tab == session.id {
                    *tab = id;
                }
            }
            session.id = id;
        }
        self.store = Some(store);
        self
    }

    /// Starts reading the saved sessions in the background; they land in [`pump`](Self::pump)
    /// after `done` wakes the window. Without a store there is nothing to read.
    pub fn load_history(&self, done: Notifier) {
        if let Some(store) = &self.store {
            store.load(done);
        }
    }

    /// Lists the saved sessions below the ones already here (sessions made this launch are
    /// newer), keeping the selection. Files that couldn't be read are reported in the notice.
    fn land_history(&mut self, history: History) -> bool {
        let before = self.sessions.len();
        for session in history.sessions {
            if self.session(session.id).is_none() {
                self.read_images(&session);
                self.sessions.push(session);
            }
        }
        let mut changed = self.sessions.len() != before;
        if let Some(first) = history.failures.first() {
            for failure in &history.failures {
                eprintln!("krampsta: {failure}");
            }
            self.notice = Some(match history.failures.len() {
                1 => first.clone(),
                n => format!("{n} saved sessions couldn't be read; the first: {first}"),
            });
            changed = true;
        }
        changed
    }

    /// Queues the session for saving, if there is a store and it has something worth keeping.
    fn persist(&self, id: SessionId) {
        if let (Some(store), Some(session)) = (&self.store, self.session(id))
            && !session.messages.is_empty()
        {
            store.save(session.clone());
        }
    }

    /// A fresh app running what `catalog` offers, starting from its initial selection, on one
    /// new, empty session (the saved history lands below it once read).
    pub fn new(catalog: Arc<dyn EngineCatalog>) -> Self {
        let ids = IdGen::new();
        let sessions = vec![Session::new(SessionId(ids.next()))];
        let selected = sessions[0].id;
        let mut views = HashMap::new();
        let mut first = SessionView::default();
        first.draft.focused = true;
        views.insert(selected, first);
        let engine = EngineChoice::new(catalog.providers(), catalog.initial());
        // A stand-in until `rebuild_engine` below. With no usable selection sends are refused,
        // so it never runs.
        let agent = AgentHost::new(Arc::new(MockAgent::new(MockConfig::default())));
        let mut app = KrampstaApp {
            sessions,
            selected,
            tabs: vec![selected],
            tab_drag: TabDragState::default(),
            split: None,
            views,
            blank_draft: TextEdit::new(""),
            notice: None,
            screen: Screen::Chat,
            theme: ThemeChoice::Dark,
            sidebar: PaneState::new(264.0).range(200.0, 420.0),
            changes: PaneState::new(440.0).range(320.0, 760.0),
            diff: empty_diff(),
            review: Changes::default(),
            approvals: VecDeque::new(),
            engine,
            sign_in: SignIn::Closed,
            api_key: KeyField::default(),
            card_diffs: HashMap::new(),
            open_tools: HashSet::new(),
            commands_with_pending_changes: HashSet::new(),
            open_reasoning: HashSet::new(),
            menu: None,
            confirm_delete: None,
            catalog,
            agent,
            store: None,
            project: None,
            collapsed_groups: HashSet::new(),
            opening_project: None,
            prices: BTreeMap::new(),
            price_fields: Vec::new(),
            price_scratch: TextEdit::new(""),
            prefs: PrefsSync::default(),
            opener: None,
            inspector: crate::files::Inspector::default(),
            files: crate::files::ProjectTree::default(),
            repo: crate::git::RepoView::default(),
            servers: crate::servers::ServerList::default(),
            images: crate::images::Images::default(),
            ids,
            window_width: None,
            auto_hidden: AutoHidden::default(),
            keep: Pane::Sidebar,
        };
        app.keep_selection_usable();
        app.rebuild_engine();
        // Where it starts isn't a change to save.
        app.prefs = PrefsSync::starting_at(app.prefs_now());
        app
    }

    /// The default app: the mock agent at its normal pace. `AppSpec::init` needs a plain `fn`.
    pub fn with_mock() -> Self {
        Self::new(Arc::new(MockCatalog::new(MockConfig::default())))
    }

    /// Wakes the window when agent events arrive.
    pub fn set_notifier(&mut self, notify: Notifier) {
        self.set_images_notifier(notify.clone());
        self.set_servers_notifier(notify.clone());
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

    /// Opens a closed tool card, closes an open one.
    pub fn toggle_tool(&mut self, id: &ToolCallId) {
        if !self.open_tools.remove(id) {
            self.open_tools.insert(id.clone());
        }
    }

    /// Opens a message's folded reasoning, or folds it again.
    pub fn toggle_reasoning(&mut self, id: MessageId) {
        if !self.open_reasoning.remove(&id) {
            self.open_reasoning.insert(id);
        }
    }

    /// Adds an empty session at the top and selects it.
    pub fn new_session(&mut self) -> SessionId {
        let id = SessionId(self.ids.next());
        let mut session = Session::new(id);
        session.project = self.project.as_ref().map(|p| p.root.clone());
        // The new session shows: its project's sessions unfold.
        self.collapsed_groups.remove(&session.project);
        self.sessions.insert(0, session);
        self.select(id);
        id
    }

    pub fn select(&mut self, id: SessionId) {
        if self.session(id).is_some() {
            self.open_tab(id);
            self.swap_if_other(id);
            self.selected = id;
            self.screen = Screen::Chat;
            // Say up front when it can't go on here, rather than on the next send.
            self.notice = self.foreign_project(id).map(|name| SendError::OtherProject(name).to_string());
            self.focus_composer(id);
        }
    }

    /// The name of the project `id` works in, if that isn't the open one.
    pub(crate) fn foreign_project(&self, id: SessionId) -> Option<String> {
        let theirs = self.session(id)?.project.as_deref()?;
        let open = self.project.as_ref().map(|p| p.root.as_path());
        (open != Some(theirs)).then(|| krampsta_agent::project_name(theirs))
    }

    /// Sends the composer's text to the agent in the selected session.
    pub fn send(&mut self) -> Result<RunId, SendError> {
        if let Some(problem) = &self.engine.problem {
            return Err(SendError::Engine(problem.clone()));
        }
        let run = self.agent.next_run_id();
        let id = self.current().map(|s| s.id).ok_or(SessionError::EmptyMessage)?;
        let text = self.draft_of(id).text.clone();
        if let Some(name) = self.foreign_project(id) {
            return Err(SendError::OtherProject(name));
        }
        let open = self.project.as_ref().map(|p| p.root.clone());
        let Selection { provider, model } = self.engine.selection.clone();
        let image = self.check_attachment(id).map_err(SendError::Engine)?;
        let session = self.session_mut(id).ok_or(SessionError::EmptyMessage)?;
        let message = session.submit_with_image(&text, image, run)?;
        // A session not tied to a folder yet works in the open one from now on.
        if session.project.is_none() {
            session.project = open;
        }
        // Its tokens are counted against the model it runs with, for what it costs.
        if let Some(model) = &model {
            session.bill_to(&provider, model);
        }
        let request =
            RunRequest { session: id, run, history: session.messages.clone(), effort: self.engine.effort.clone() };
        self.sent_attachment(id, run, message, text);
        self.view_mut(id).draft = TextEdit::new("");
        self.focus_composer(id);
        self.notice = None;
        let started = self.agent.start(request);
        if let Err(e) = &started {
            if let Some(s) = self.session_mut(id) {
                s.apply(&AgentEvent::Failed { run, error: e.to_string() });
            }
            self.finish_image_send(id, run, true);
        }
        self.persist(id);
        started.map_err(SendError::from)
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
        let writes = self.agent.drain_writes();
        let projects = self.agent.drain_projects();
        let file_lists = self.agent.drain_files();
        let repo_diffs = self.agent.drain_repo();
        let mut changed = self.land_accounts(accounts);
        changed |= self.land_projects(projects);
        changed |= self.land_files(file_lists);
        changed |= self.land_repo(repo_diffs);
        changed |= self.land_models(lists);
        let (writes_changed, mut panel_dirty) = self.land_writes(writes);
        changed |= writes_changed;
        changed |= self.land_images();
        changed |= self.land_servers();
        for e in self.agent.drain() {
            match e.event {
                // A proposal outlives its run: the tools already build on it.
                AgentEvent::ChangeProposed { call, change, .. } => {
                    // Its card shows it (worked out once, here, never in the view).
                    self.card_diffs.insert(call, crate::changes::diff_of(&change));
                    panel_dirty |= self.stage_proposal(change);
                    changed = true;
                }
                AgentEvent::ApprovalRequested { run, call, tool, what } => {
                    self.ask(Asking { run, call, tool, what });
                    changed = true;
                }
                event => {
                    if let AgentEvent::ToolCallStarted { run, call } = &event
                        && call.name == "run_command"
                        && !self.review.proposals().is_empty()
                    {
                        self.commands_with_pending_changes.insert((*run, call.id.clone()));
                    }
                    if event.is_terminal() {
                        self.forget_approvals(event.run());
                    }
                    let applied = self.session_mut(e.session).is_some_and(|s| s.apply(&event));
                    changed |= applied;
                    if applied && event.is_terminal() {
                        self.finish_image_send(e.session, event.run(), matches!(event, AgentEvent::Failed { .. }));
                    }
                    // Saved at each step's end, not per word: a crash mid-run keeps the
                    // finished steps, and no frame writes.
                    let step_ended = matches!(event, AgentEvent::ToolCallFinished { .. }) || event.is_terminal();
                    if applied && step_ended {
                        self.persist(e.session);
                    }
                }
            }
        }
        changed |= self.finish_apply_and_validate();
        if let Some(history) = self.store.as_ref().and_then(|s| s.take_history()) {
            changed |= self.land_history(history);
        }
        if let Some(failure) = self.store.as_ref().and_then(|s| s.drain_failures().pop()) {
            self.notice = Some(failure);
            changed = true;
        }
        if panel_dirty {
            self.refresh_diff();
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

    /// Fits the panes to the window again after the conversation area changed (a split).
    pub(crate) fn refit(&mut self) {
        if let Some(w) = self.window_width {
            self.fit_width(w);
        }
    }

    /// Keeps the conversation readable as the window narrows: side panes close (the one the
    /// user opened last closes last) and reopen once there is room again. Runs whenever the
    /// window's width changes.
    pub fn fit_width(&mut self, width: f32) {
        self.window_width = Some(width);
        // Two conversations side by side need more room than one (each a little under one).
        let panes = if self.split.is_some() { SPLIT_ROOM } else { 1.0 };
        let needed_for = move |main: f32, side: Option<f32>, changes: Option<f32>| {
            main * panes + PANE_SLACK + side.unwrap_or(0.0) + changes.unwrap_or(0.0)
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
/// How many single conversations' worth of room a split takes.
pub const SPLIT_ROOM: f32 = 1.6;
/// Frame insets and pane padding around the panes' own widths.
const PANE_SLACK: f32 = 40.0;

/// The changes panel before anything is proposed; long lines wrap in its narrow column.
fn empty_diff() -> DiffState {
    let mut diff = DiffState::new(Vec::new());
    diff.set_wrap(true);
    diff
}

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
    fn starts_on_one_new_empty_session_with_no_examples() {
        let app = app();
        assert_eq!(app.sessions.len(), 1);
        assert!(app.sessions[0].messages.is_empty());
        assert_eq!(app.current().map(|s| s.id), Some(app.selected));

        let samples = app.with_sample_sessions();
        assert!(samples.sessions.len() > 1, "stills and tests ask for the examples");
        assert_eq!(samples.current().map(|s| s.id), Some(samples.selected), "the first is selected");
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
        app.set_draft("Refactor the parser");
        app.send().unwrap();
        assert!(app.draft().text.is_empty(), "the composer clears");
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
    fn sessions_are_saved_once_they_have_a_message_and_again_when_the_run_ends() {
        let store = Arc::new(krampsta_agent::MemoryStore::new());
        let mut app = app().with_store(store.clone());
        assert!(app.sessions.iter().all(|s| s.id.0 > 1_600_000_000_000), "clock ids, not 1, 2…");
        assert_eq!(app.current().map(|s| s.id), Some(app.selected), "the selection follows the new ids");
        assert!(app.draft().focused, "and so does the composer's caret");

        let id = app.new_session();
        assert!(store.sessions().is_empty(), "an empty session isn't saved");
        app.set_draft("Refactor the parser");
        app.send().unwrap();
        assert_eq!(store.sessions()[0].status, AgentStatus::Running, "saved as soon as it was sent");
        pump_until_idle(&mut app);
        let saved = store.sessions();
        assert_eq!(saved.len(), 1);
        assert_eq!(Some(&saved[0]), app.session(id), "the finished run, reply included");

        app.ask_delete(id);
        app.confirm_delete();
        assert!(store.sessions().is_empty(), "deleting a session deletes what was saved");
    }

    /// A session an earlier launch saved: a question and its answer.
    fn saved_session(id: u64, title: &str) -> Session {
        let mut s = Session::with_title(SessionId(id), title);
        s.push_message(MessageRole::User, "Where is the parser?");
        s.push_message(MessageRole::Assistant, "In `src/parse.rs`.");
        s.status = AgentStatus::Completed;
        s
    }

    #[test]
    fn saved_sessions_land_below_the_open_ones_and_keep_the_selection() {
        let older = saved_session(1_700_000_000_000, "Older");
        let newer = saved_session(1_700_000_000_500, "Newer");
        let store = Arc::new(krampsta_agent::MemoryStore::with_sessions(vec![newer.clone(), older.clone()]));
        let mut app = app().with_store(store);
        let (open, selected) = (app.sessions.len(), app.selected);
        assert!(app.session(newer.id).is_none(), "nothing is read before load_history");

        app.load_history(Arc::new(|| {}));
        assert!(app.pump(), "landing the history is a visible change");
        assert_eq!(app.sessions.len(), open + 2);
        assert_eq!(app.sessions[open].id, newer.id, "newest saved first, after the open sessions");
        assert_eq!(app.sessions[open + 1].id, older.id);
        assert_eq!(app.selected, selected);
        assert!(!app.pump(), "handed over once");

        // A saved conversation goes on where it left off.
        app.select(newer.id);
        app.set_draft("And the lexer?");
        app.send().unwrap();
        pump_until_idle(&mut app);
        let s = app.session(newer.id).unwrap();
        assert_eq!(s.messages.len(), 4);
        let ids: HashSet<MessageId> = s.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), 4, "new messages don't reuse saved ids");
    }

    #[test]
    fn unreadable_saved_sessions_are_reported_in_the_notice() {
        let mut app = app();
        assert!(app.land_history(History { sessions: Vec::new(), failures: vec!["ses9.json is damaged".into()] }));
        assert_eq!(app.notice.as_deref(), Some("ses9.json is damaged"));
        let failures = vec!["a".into(), "b".into()];
        app.land_history(History { sessions: Vec::new(), failures });
        assert_eq!(app.notice.as_deref(), Some("2 saved sessions couldn't be read; the first: a"));
    }

    #[test]
    fn blank_drafts_are_not_sent() {
        let mut app = app();
        app.set_draft("   ");
        assert!(matches!(app.send(), Err(SendError::Session(SessionError::EmptyMessage))));
        app.submit();
        assert_eq!(app.notice, None, "an empty send is silently ignored");
    }

    #[test]
    fn fail_command_shows_the_failed_state() {
        let mut app = app();
        app.set_draft("/fail now");
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
        app.set_draft("take your time");
        app.send().unwrap();
        assert!(app.cancel());
        pump_until_idle(&mut app);
        assert_eq!(app.current().map(|s| s.status.clone()), Some(AgentStatus::Cancelled));
        assert!(!app.cancel(), "nothing left to cancel");
    }
}
