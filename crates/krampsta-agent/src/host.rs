//! The run host: drives engines on a background async runtime and hands their events to the UI.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use krampsta_core::{AgentEvent, IdGen, RunEvent, RunId, ToolCallId};
use tokio_util::sync::CancellationToken;

use crate::catalog::{DeviceCode, LoginPrompt};
use crate::engine::{AgentEngine, AgentError, Approvals, BoxFuture, RunContext, RunRequest, lock};
use crate::projects::{ProjectOpened, ProjectOpener};
use crate::tools::{Decision, ProjectFiles, RepoDiff, Toolbox};

/// Called from a worker thread after each event is queued, to wake the UI so it drains them.
/// Rune's `Waker` (`Arc<dyn Fn() + Send + Sync>`) is one.
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

/// Owns the engine, the async runtime and the event channel.
///
/// The UI thread calls [`AgentHost::start`] and [`AgentHost::cancel`] (both return at once)
/// and drains events with [`AgentHost::drain`] when the notifier fires. The runtime is created
/// on the first run, so an app that never sends a message never starts its threads.
pub struct AgentHost {
    engine: Arc<dyn AgentEngine>,
    /// The tools runs started from now on may call.
    tools: Option<Arc<dyn Toolbox>>,
    runtime: Option<tokio::runtime::Runtime>,
    events_tx: Sender<RunEvent>,
    events_rx: Receiver<RunEvent>,
    notify: Notifier,
    runs: HashMap<RunId, CancellationToken>,
    ids: IdGen,
    lookups_tx: Sender<ModelList>,
    lookups_rx: Receiver<ModelList>,
    accounts_tx: Sender<AccountEvent>,
    accounts_rx: Receiver<AccountEvent>,
    writes_tx: Sender<Written>,
    writes_rx: Receiver<Written>,
    projects_tx: Sender<ProjectOpened>,
    projects_rx: Receiver<ProjectOpened>,
    files_tx: Sender<FileList>,
    files_rx: Receiver<FileList>,
    repo_tx: Sender<RepoDiffList>,
    repo_rx: Receiver<RepoDiffList>,
    /// Calls waiting for the user to allow them, shared with every run's context.
    approvals: Approvals,
    /// The sign-in in progress, if any (one at a time).
    sign_in: Option<CancellationToken>,
}

/// The answer to [`AgentHost::write_file`]: what was asked, and how it went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub path: String,
    pub content: Arc<str>,
    pub result: Result<(), String>,
}

/// The answer to [`AgentHost::list_files`]: which request it answers, and the files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileList {
    /// The number the caller gave the request, to tell a stale answer (from a project
    /// that was open before) from the current one.
    pub ask: u64,
    pub result: Result<ProjectFiles, String>,
}

/// The answer to [`AgentHost::diff_repo`]: which request it answers, and the diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoDiffList {
    /// The number the caller gave the request (see [`FileList::ask`]).
    pub ask: u64,
    pub result: Result<RepoDiff, String>,
}

/// The answer to [`AgentHost::fetch_models`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelList {
    pub provider: String,
    pub models: Result<Vec<String>, AgentError>,
}

/// What a sign-in or sign-out reports, through [`AgentHost::drain_accounts`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountEvent {
    /// The sign-in has a code for the user to enter.
    Code {
        provider: String,
        code: DeviceCode,
    },
    /// The sign-in ended: the account's name, or why not (`Cancelled` when the user gave up).
    SignedIn {
        provider: String,
        result: Result<String, AgentError>,
    },
    SignedOut {
        provider: String,
        result: Result<(), AgentError>,
    },
    /// An API key was saved or forgotten.
    KeySaved {
        provider: String,
        result: Result<(), AgentError>,
    },
}

impl AgentHost {
    pub fn new(engine: Arc<dyn AgentEngine>) -> Self {
        let (events_tx, events_rx) = channel();
        let (lookups_tx, lookups_rx) = channel();
        let (accounts_tx, accounts_rx) = channel();
        let (writes_tx, writes_rx) = channel();
        let (projects_tx, projects_rx) = channel();
        let (files_tx, files_rx) = channel();
        let (repo_tx, repo_rx) = channel();
        AgentHost {
            writes_tx,
            writes_rx,
            projects_tx,
            projects_rx,
            files_tx,
            files_rx,
            repo_tx,
            repo_rx,
            approvals: Approvals::default(),
            engine,
            tools: None,
            runtime: None,
            events_tx,
            events_rx,
            notify: Arc::new(|| {}),
            runs: HashMap::new(),
            ids: IdGen::new(),
            lookups_tx,
            lookups_rx,
            accounts_tx,
            accounts_rx,
            sign_in: None,
        }
    }

    /// Sets what wakes the UI when events arrive (runs started later use it).
    pub fn set_notifier(&mut self, notify: Notifier) {
        self.notify = notify;
    }

    pub fn engine_name(&self) -> &str {
        self.engine.name()
    }

    /// Runs started from now on use `engine`; runs already going keep theirs.
    pub fn set_engine(&mut self, engine: Arc<dyn AgentEngine>) {
        self.engine = engine;
    }

    /// Runs started from now on may call `tools` (none: the model gets no tools).
    pub fn set_toolbox(&mut self, tools: Option<Arc<dyn Toolbox>>) {
        self.tools = tools;
    }

    /// Writes an accepted change in the background, through the tools (which check the file
    /// still holds `expected`); the outcome arrives through [`drain_writes`](Self::drain_writes).
    pub fn write_file(&mut self, path: &str, expected: Option<Arc<str>>, content: Arc<str>) -> Result<(), AgentError> {
        let tools = self.tools.clone().ok_or_else(|| AgentError::Runtime("there is no project to write to".into()))?;
        let (tx, notify, path) = (self.writes_tx.clone(), self.notify.clone(), path.to_owned());
        self.runtime()?.spawn(async move {
            let job = {
                let (path, content) = (path.clone(), content.clone());
                tokio::task::spawn_blocking(move || tools.write(&path, expected.as_deref(), &content))
            };
            let result = job.await.unwrap_or_else(|e| Err(format!("the write stopped abnormally: {e}")));
            if tx.send(Written { path, content, result }).is_ok() {
                notify();
            }
        });
        Ok(())
    }

    /// Takes every finished write, without blocking.
    pub fn drain_writes(&mut self) -> Vec<Written> {
        self.writes_rx.try_iter().collect()
    }

    /// Opens `path` as a project with `opener`, off the UI thread; the answer arrives through
    /// [`drain_projects`](Self::drain_projects). The tools runs use don't change until the
    /// caller hands them to [`set_toolbox`](Self::set_toolbox).
    pub fn open_project(&mut self, opener: Arc<dyn ProjectOpener>, path: std::path::PathBuf) -> Result<(), AgentError> {
        let (tx, notify) = (self.projects_tx.clone(), self.notify.clone());
        self.runtime()?.spawn(async move {
            let asked = path.clone();
            let job = tokio::task::spawn_blocking(move || opener.open(&asked));
            let result = job.await.unwrap_or_else(|e| Err(format!("opening the folder stopped abnormally: {e}")));
            if tx.send(ProjectOpened { path, result }).is_ok() {
                notify();
            }
        });
        Ok(())
    }

    /// Lists the project's files through the tools, off the UI thread; the answer, tagged
    /// `ask`, arrives through [`drain_files`](Self::drain_files).
    pub fn list_files(&mut self, ask: u64) -> Result<(), AgentError> {
        let (tx, notify, tools) = (self.files_tx.clone(), self.notify.clone(), self.tools.clone());
        let Some(tools) = tools else {
            // Nothing to wait for: the answer is already known.
            if tx.send(FileList { ask, result: Err("no project folder is open".into()) }).is_ok() {
                notify();
            }
            return Ok(());
        };
        self.runtime()?.spawn(async move {
            let job = tokio::task::spawn_blocking(move || tools.files());
            let result = job.await.unwrap_or_else(|e| Err(format!("listing the files stopped abnormally: {e}")));
            if tx.send(FileList { ask, result }).is_ok() {
                notify();
            }
        });
        Ok(())
    }

    /// Takes every answer to [`list_files`](Self::list_files), without blocking.
    pub fn drain_files(&mut self) -> Vec<FileList> {
        self.files_rx.try_iter().collect()
    }

    /// Reads what differs from the repository's last commit through the tools, off the UI
    /// thread; the answer, tagged `ask`, arrives through [`drain_repo`](Self::drain_repo).
    pub fn diff_repo(&mut self, ask: u64) -> Result<(), AgentError> {
        let (tx, notify, tools) = (self.repo_tx.clone(), self.notify.clone(), self.tools.clone());
        let Some(tools) = tools else {
            if tx.send(RepoDiffList { ask, result: Err("no project folder is open".into()) }).is_ok() {
                notify();
            }
            return Ok(());
        };
        self.runtime()?.spawn(async move {
            let job = tokio::task::spawn_blocking(move || tools.repo_diff());
            let result = job.await.unwrap_or_else(|e| Err(format!("reading the repository stopped abnormally: {e}")));
            if tx.send(RepoDiffList { ask, result }).is_ok() {
                notify();
            }
        });
        Ok(())
    }

    /// Takes every answer to [`diff_repo`](Self::diff_repo), without blocking.
    pub fn drain_repo(&mut self) -> Vec<RepoDiffList> {
        self.repo_rx.try_iter().collect()
    }

    /// Takes every answer to [`open_project`](Self::open_project), without blocking.
    pub fn drain_projects(&mut self) -> Vec<ProjectOpened> {
        self.projects_rx.try_iter().collect()
    }

    /// What the agent's next edits and reads of `path` build on (see [`Toolbox::set_pending`]).
    /// Cheap: no I/O.
    pub fn set_pending(&self, path: &str, content: Option<Arc<str>>) {
        if let Some(tools) = &self.tools {
            tools.set_pending(path, content);
        }
    }

    /// Resolves `lookup` (a catalog's model list) in the background; the answer arrives through
    /// [`drain_models`](Self::drain_models) and wakes the UI like a run event.
    pub fn fetch_models(
        &mut self,
        provider: &str,
        lookup: BoxFuture<'static, Result<Vec<String>, AgentError>>,
    ) -> Result<(), AgentError> {
        let (tx, notify, provider) = (self.lookups_tx.clone(), self.notify.clone(), provider.to_owned());
        self.runtime()?.spawn(async move {
            let models = lookup.await;
            if tx.send(ModelList { provider, models }).is_ok() {
                notify();
            }
        });
        Ok(())
    }

    /// Takes every model list that has arrived, without blocking.
    pub fn drain_models(&mut self) -> Vec<ModelList> {
        self.lookups_rx.try_iter().collect()
    }

    /// Starts a device sign-in in the background, replacing (cancelling) one already going.
    /// `login` receives the prompt to call with the code (a catalog's `login`). Its code, then
    /// its outcome, arrive through [`drain_accounts`](Self::drain_accounts).
    pub fn sign_in(
        &mut self,
        provider: &str,
        login: impl FnOnce(LoginPrompt) -> BoxFuture<'static, Result<String, AgentError>>,
    ) -> Result<(), AgentError> {
        self.cancel_sign_in();
        let (tx, notify) = (self.accounts_tx.clone(), self.notify.clone());
        let send = move |event: AccountEvent| {
            if tx.send(event).is_ok() {
                notify();
            }
        };
        let id = provider.to_owned();
        let prompt_send = send.clone();
        let prompt: LoginPrompt = Arc::new(move |code| prompt_send(AccountEvent::Code { provider: id.clone(), code }));
        let work = login(prompt);
        let cancel = CancellationToken::new();
        let (token, provider) = (cancel.clone(), provider.to_owned());
        self.runtime()?.spawn(async move {
            let result = tokio::select! {
                biased;
                () = token.cancelled() => Err(AgentError::Cancelled),
                result = work => result,
            };
            send(AccountEvent::SignedIn { provider, result });
        });
        self.sign_in = Some(cancel);
        Ok(())
    }

    /// Stops the sign-in in progress; it still reports `SignedIn` with `Err(Cancelled)`.
    pub fn cancel_sign_in(&mut self) -> bool {
        self.sign_in.take().map(|t| t.cancel()).is_some()
    }

    /// Runs `logout` (a catalog's) in the background; reports `SignedOut`.
    pub fn sign_out(
        &mut self,
        provider: &str,
        logout: BoxFuture<'static, Result<(), AgentError>>,
    ) -> Result<(), AgentError> {
        let provider = provider.to_owned();
        self.report(logout, move |result| AccountEvent::SignedOut { provider, result })
    }

    /// Runs `save` (a catalog's `set_api_key`) in the background; reports `KeySaved`.
    pub fn save_api_key(
        &mut self,
        provider: &str,
        save: BoxFuture<'static, Result<(), AgentError>>,
    ) -> Result<(), AgentError> {
        let provider = provider.to_owned();
        self.report(save, move |result| AccountEvent::KeySaved { provider, result })
    }

    /// Runs `work` in the background, then sends the event `event` makes of its outcome.
    fn report(
        &mut self,
        work: BoxFuture<'static, Result<(), AgentError>>,
        event: impl FnOnce(Result<(), AgentError>) -> AccountEvent + Send + 'static,
    ) -> Result<(), AgentError> {
        let (tx, notify) = (self.accounts_tx.clone(), self.notify.clone());
        self.runtime()?.spawn(async move {
            if tx.send(event(work.await)).is_ok() {
                notify();
            }
        });
        Ok(())
    }

    /// Takes every account event that has arrived, without blocking.
    pub fn drain_accounts(&mut self) -> Vec<AccountEvent> {
        let events: Vec<AccountEvent> = self.accounts_rx.try_iter().collect();
        if events.iter().any(|e| matches!(e, AccountEvent::SignedIn { .. })) {
            self.sign_in = None;
        }
        events
    }

    /// A fresh run id, to reserve a session before [`AgentHost::start`].
    pub fn next_run_id(&self) -> RunId {
        RunId(self.ids.next())
    }

    /// Whether `run` has started and not yet delivered its terminal event to [`drain`](Self::drain).
    pub fn is_active(&self, run: RunId) -> bool {
        self.runs.contains_key(&run)
    }

    fn runtime(&mut self) -> Result<&tokio::runtime::Runtime, AgentError> {
        if self.runtime.is_none() {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("krampsta-agent")
                .enable_all()
                .build()
                .map_err(|e| AgentError::Runtime(e.to_string()))?;
            self.runtime = Some(rt);
        }
        self.runtime.as_ref().ok_or_else(|| AgentError::Runtime("runtime unavailable".into()))
    }

    /// Starts a run in the background. Its events (beginning with `Started`, ending with one
    /// terminal event) arrive through [`drain`](Self::drain).
    pub fn start(&mut self, request: RunRequest) -> Result<RunId, AgentError> {
        let run = request.run;
        let cancel = CancellationToken::new();
        let ctx = RunContext::new(
            request.session,
            run,
            self.events_tx.clone(),
            self.notify.clone(),
            cancel.clone(),
            self.tools.clone(),
            self.approvals.clone(),
        );
        let engine = self.engine.clone();
        let runtime = self.runtime()?;
        let handle = runtime.handle().clone();
        runtime.spawn(async move {
            ctx.emit(AgentEvent::Started { run });
            // A long conversation is summed up first (`compact.rs`).
            let result = match crate::compact::before_run(engine.clone(), request, &ctx, &handle).await {
                // The engine runs as its own task so a panic inside it still ends the run.
                Ok(request) => match handle.spawn(engine.run(request, ctx.clone())).await {
                    Ok(result) => result,
                    Err(join) => Err(AgentError::Provider(format!("engine task ended abnormally: {join}"))),
                },
                Err(stopped) => Err(stopped),
            };
            let terminal = match result {
                _ if ctx.cancellation().is_cancelled() => AgentEvent::Cancelled { run },
                Ok(()) => AgentEvent::Completed { run },
                Err(AgentError::Cancelled) => AgentEvent::Cancelled { run },
                Err(e) => AgentEvent::Failed { run, error: e.to_string() },
            };
            ctx.emit(terminal);
        });
        self.runs.insert(run, cancel);
        Ok(run)
    }

    /// Answers a call waiting for approval ([`AgentEvent::ApprovalRequested`]). False when it no
    /// longer waits (its run ended, or it was answered).
    pub fn answer(&self, run: RunId, call: &ToolCallId, decision: Decision) -> bool {
        match lock(&self.approvals).remove(&(run, call.clone())) {
            Some(waiting) => waiting.send(decision).is_ok(),
            None => false,
        }
    }

    /// Asks a run to stop. Returns false if it isn't running. The run still ends with a
    /// `Cancelled` event through [`drain`](Self::drain).
    pub fn cancel(&mut self, run: RunId) -> bool {
        match self.runs.get(&run) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Takes every event that has arrived, without blocking.
    pub fn drain(&mut self) -> Vec<RunEvent> {
        let events: Vec<RunEvent> = self.events_rx.try_iter().collect();
        for e in &events {
            if e.event.is_terminal() {
                self.runs.remove(&e.event.run());
            }
        }
        events
    }
}

impl Drop for AgentHost {
    fn drop(&mut self) {
        self.cancel_sign_in();
        for token in self.runs.values() {
            token.cancel();
        }
        // Never block the UI thread waiting for workers on shutdown.
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::BoxFuture;
    use krampsta_core::{Message, MessageId, SessionId};
    use std::time::{Duration, Instant};

    struct Scripted(Result<(), AgentError>);

    impl AgentEngine for Scripted {
        fn name(&self) -> &str {
            "scripted"
        }
        fn run(self: Arc<Self>, _: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
            Box::pin(async move {
                ctx.text("hello");
                self.0.clone()
            })
        }
    }

    struct Panics;

    impl AgentEngine for Panics {
        fn name(&self) -> &str {
            "panics"
        }
        #[allow(clippy::panic)]
        fn run(self: Arc<Self>, _: RunRequest, _: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
            Box::pin(async move { panic!("engine bug") })
        }
    }

    fn request(host: &AgentHost) -> RunRequest {
        RunRequest {
            session: SessionId(1),
            run: host.next_run_id(),
            history: vec![Message::user(MessageId(1), "hi")],
            effort: None,
        }
    }

    /// Drains until a terminal event arrives (or fails after 5 s).
    fn events_until_done(host: &mut AgentHost) -> Vec<AgentEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut all = Vec::new();
        loop {
            all.extend(host.drain().into_iter().map(|e| e.event));
            if all.last().is_some_and(AgentEvent::is_terminal) {
                return all;
            }
            assert!(Instant::now() < deadline, "run never finished: {all:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_run_is_framed_by_started_and_completed() {
        let mut host = AgentHost::new(Arc::new(Scripted(Ok(()))));
        let run = host.start(request(&host)).unwrap();
        assert!(host.is_active(run));
        let events = events_until_done(&mut host);
        assert_eq!(events.first(), Some(&AgentEvent::Started { run }));
        assert_eq!(events.get(1), Some(&AgentEvent::TextDelta { run, text: "hello".into() }));
        assert_eq!(events.last(), Some(&AgentEvent::Completed { run }));
        assert!(!host.is_active(run));
    }

    #[test]
    fn engine_errors_become_failed_events() {
        let mut host = AgentHost::new(Arc::new(Scripted(Err(AgentError::Provider("quota".into())))));
        let run = host.start(request(&host)).unwrap();
        let events = events_until_done(&mut host);
        assert_eq!(events.last(), Some(&AgentEvent::Failed { run, error: "provider error: quota".into() }));
    }

    #[test]
    fn a_panicking_engine_still_ends_its_run() {
        let mut host = AgentHost::new(Arc::new(Panics));
        let run = host.start(request(&host)).unwrap();
        let events = events_until_done(&mut host);
        assert!(matches!(events.last(), Some(AgentEvent::Failed { run: r, .. }) if *r == run));
    }

    #[test]
    fn switching_engines_affects_only_new_runs() {
        let mut host = AgentHost::new(Arc::new(Scripted(Ok(()))));
        host.set_engine(Arc::new(Scripted(Err(AgentError::Provider("other".into())))));
        assert_eq!(host.engine_name(), "scripted");
        host.start(request(&host)).unwrap();
        assert!(matches!(events_until_done(&mut host).last(), Some(AgentEvent::Failed { .. })));
    }

    #[test]
    fn model_lookups_arrive_through_drain_models() {
        let mut host = AgentHost::new(Arc::new(Scripted(Ok(()))));
        host.fetch_models("p", Box::pin(async { Ok(vec!["m1".to_owned()]) })).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut lists = Vec::new();
        while lists.is_empty() {
            assert!(Instant::now() < deadline, "the lookup never arrived");
            lists = host.drain_models();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(lists, [ModelList { provider: "p".into(), models: Ok(vec!["m1".into()]) }]);
    }

    #[test]
    fn the_notifier_fires_for_every_event() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut host = AgentHost::new(Arc::new(Scripted(Ok(()))));
        let c = count.clone();
        host.set_notifier(Arc::new(move || {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        host.start(request(&host)).unwrap();
        let events = events_until_done(&mut host);
        // The notifier runs just after each send, so give the last one a moment to land.
        let deadline = Instant::now() + Duration::from_secs(2);
        while count.load(std::sync::atomic::Ordering::SeqCst) < events.len() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), events.len());
    }

    /// Calls `run_command` once and writes back what it returned.
    struct CallsATool;

    impl AgentEngine for CallsATool {
        fn name(&self) -> &str {
            "calls-a-tool"
        }
        fn run(self: Arc<Self>, _: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
            Box::pin(async move {
                let call = krampsta_core::ToolCall {
                    id: ToolCallId("c1".into()),
                    name: "run_command".into(),
                    arguments: "ls".into(),
                };
                let result = ctx.run_tool(call).await?;
                ctx.text(format!("{}:{}", result.is_error, result.output));
                Ok(())
            })
        }
    }

    /// Asks about every call; shows output as it runs; answers with how it was approved.
    #[derive(Default)]
    struct Asks {
        remembered: std::sync::Mutex<Vec<String>>,
    }

    impl Toolbox for Asks {
        fn specs(&self) -> Arc<[crate::ToolSpec]> {
            Arc::from([])
        }
        fn check(&self, _: &str, arguments: &str) -> crate::Access {
            crate::Access::Ask { what: arguments.to_owned() }
        }
        fn remember(&self, _: &str, arguments: &str) -> Result<(), String> {
            self.remembered.lock().unwrap().push(arguments.to_owned());
            Ok(())
        }
        fn run(&self, _: &str, _: &str, call: &crate::Call<'_>) -> Result<crate::ToolOutput, String> {
            (call.output)("partial ");
            Ok(format!("ran {:?}", call.approval).into())
        }
    }

    fn ask(tools: &Arc<Asks>) -> (AgentHost, RunId) {
        let mut host = AgentHost::new(Arc::new(CallsATool));
        host.set_toolbox(Some(tools.clone()));
        let run = host.next_run_id();
        host.start(RunRequest { session: SessionId(1), run, history: vec![], effort: None }).unwrap();
        (host, run)
    }

    fn until(host: &mut AgentHost, events: &mut Vec<AgentEvent>, done: impl Fn(&[AgentEvent]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(events) {
            assert!(Instant::now() < deadline, "timed out: {events:?}");
            events.extend(host.drain().into_iter().map(|e| e.event));
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn asked(events: &[AgentEvent]) -> bool {
        events.iter().any(|e| matches!(e, AgentEvent::ApprovalRequested { what, .. } if what == "ls"))
    }

    fn reply(events: &[AgentEvent]) -> String {
        events
            .iter()
            .filter_map(|e| if let AgentEvent::TextDelta { text, .. } = e { Some(text.as_str()) } else { None })
            .collect()
    }

    #[test]
    fn a_call_that_asks_waits_for_the_answer_and_runs_as_allowed() {
        for (decision, expected) in [(Decision::Once, "false:ran Once"), (Decision::Always, "false:ran Always")] {
            let tools = Arc::new(Asks::default());
            let (mut host, run) = ask(&tools);
            let mut events = Vec::new();
            until(&mut host, &mut events, asked);
            assert!(
                !events.iter().any(|e| matches!(e, AgentEvent::TextDelta { .. })),
                "nothing runs before the answer"
            );
            assert!(host.answer(run, &ToolCallId("c1".into()), decision));
            assert!(!host.answer(run, &ToolCallId("c1".into()), decision), "answered once");
            until(&mut host, &mut events, |e| e.last().is_some_and(AgentEvent::is_terminal));
            assert_eq!(reply(&events), expected);
            let streamed =
                events.iter().any(|e| matches!(e, AgentEvent::ToolOutputDelta { text, .. } if text == "partial "));
            assert!(streamed, "output shows while the call runs");
            let remembered = tools.remembered.lock().unwrap().clone();
            assert_eq!(remembered, if decision == Decision::Always { vec!["ls".to_owned()] } else { vec![] });
        }
    }

    #[test]
    fn a_denied_call_tells_the_model_and_a_cancelled_wait_stops_the_run() {
        let tools = Arc::new(Asks::default());
        let (mut host, run) = ask(&tools);
        let mut events = Vec::new();
        until(&mut host, &mut events, asked);
        host.answer(run, &ToolCallId("c1".into()), Decision::Deny);
        until(&mut host, &mut events, |e| e.last().is_some_and(AgentEvent::is_terminal));
        assert!(reply(&events).starts_with("true:The user did not allow this"), "{}", reply(&events));

        let (mut host, run) = ask(&tools);
        let mut events = Vec::new();
        until(&mut host, &mut events, asked);
        host.cancel(run);
        until(&mut host, &mut events, |e| e.last().is_some_and(AgentEvent::is_terminal));
        assert!(matches!(events.last(), Some(AgentEvent::Cancelled { .. })));
        assert!(!host.answer(run, &ToolCallId("c1".into()), Decision::Once), "nothing waits any more");
    }

    /// Opens folders whose name starts with "ok", off whichever thread calls it.
    struct Opens;

    impl ProjectOpener for Opens {
        fn open(&self, root: &std::path::Path) -> Result<crate::OpenedProject, String> {
            if root.to_string_lossy().starts_with("ok") {
                Ok(crate::OpenedProject { root: root.to_owned(), tools: Arc::new(Asks::default()) })
            } else {
                Err(format!("{} is not a folder", root.display()))
            }
        }
    }

    #[test]
    fn projects_open_in_the_background_and_report_how_it_went() {
        let mut host = AgentHost::new(Arc::new(Scripted(Ok(()))));
        host.open_project(Arc::new(Opens), "ok-project".into()).unwrap();
        host.open_project(Arc::new(Opens), "missing".into()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut answers = Vec::new();
        while answers.len() < 2 {
            assert!(Instant::now() < deadline, "the opener never answered");
            answers.extend(host.drain_projects());
            std::thread::sleep(Duration::from_millis(2));
        }
        answers.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(answers[0].result.as_ref().unwrap_err(), "missing is not a folder");
        assert_eq!(answers[1].result.as_ref().map(|p| p.name()).unwrap(), "ok-project");
    }
}
