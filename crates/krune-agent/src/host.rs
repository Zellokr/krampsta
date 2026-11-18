//! The run host: drives engines on a background async runtime and hands their events to the UI.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use krune_core::{AgentEvent, IdGen, RunEvent, RunId};
use tokio_util::sync::CancellationToken;

use crate::engine::{AgentEngine, AgentError, BoxFuture, RunContext, RunRequest};

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
    runtime: Option<tokio::runtime::Runtime>,
    events_tx: Sender<RunEvent>,
    events_rx: Receiver<RunEvent>,
    notify: Notifier,
    runs: HashMap<RunId, CancellationToken>,
    ids: IdGen,
    lookups_tx: Sender<ModelList>,
    lookups_rx: Receiver<ModelList>,
}

/// The answer to [`AgentHost::fetch_models`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelList {
    pub provider: String,
    pub models: Result<Vec<String>, AgentError>,
}

impl AgentHost {
    pub fn new(engine: Arc<dyn AgentEngine>) -> Self {
        let (events_tx, events_rx) = channel();
        let (lookups_tx, lookups_rx) = channel();
        AgentHost {
            engine,
            runtime: None,
            events_tx,
            events_rx,
            notify: Arc::new(|| {}),
            runs: HashMap::new(),
            ids: IdGen::new(),
            lookups_tx,
            lookups_rx,
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
                .thread_name("krune-agent")
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
        let ctx = RunContext::new(request.session, run, self.events_tx.clone(), self.notify.clone(), cancel.clone());
        let engine = self.engine.clone();
        let runtime = self.runtime()?;
        let handle = runtime.handle().clone();
        runtime.spawn(async move {
            ctx.emit(AgentEvent::Started { run });
            // The engine runs as its own task so a panic inside it still ends the run.
            let result = match handle.spawn(engine.run(request, ctx.clone())).await {
                Ok(result) => result,
                Err(join) => Err(AgentError::Provider(format!("engine task ended abnormally: {join}"))),
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
    use krune_core::{Message, MessageId, SessionId};
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
        RunRequest { session: SessionId(1), run: host.next_run_id(), history: vec![Message::user(MessageId(1), "hi")] }
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
}
