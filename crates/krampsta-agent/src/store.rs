//! Where sessions are kept between launches.
//!
//! The UI calls a [`SessionStore`] from its own thread, so every method returns at once: an
//! implementation queues the work and does the I/O elsewhere. What it reads comes back through
//! [`SessionStore::take_history`] once it wakes the window; what fails, through
//! [`SessionStore::drain_failures`].

use std::sync::Mutex;

use krampsta_core::{Session, SessionId};

use crate::Notifier;

/// The saved sessions, newest first, and why any that are there couldn't be read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub sessions: Vec<Session>,
    /// One readable message per session file that couldn't be read.
    pub failures: Vec<String>,
}

/// Keeps sessions somewhere durable. Never blocks the caller.
pub trait SessionStore: Send + Sync {
    /// Starts reading every saved session in the background, and calls `done` once
    /// [`take_history`](Self::take_history) has them.
    fn load(&self, done: Notifier);

    /// The history [`load`](Self::load) read, once (None before it is ready, and after).
    fn take_history(&self) -> Option<History>;

    /// Queues `session` to be written, replacing what was saved for it. Saving the same session
    /// again before the write happens only writes the newest copy.
    fn save(&self, session: Session);

    /// Queues the session's removal.
    fn delete(&self, id: SessionId);

    /// Queues the app's settings to be written, replacing the saved ones. The UI owns their
    /// shape; the store keeps them as given.
    fn save_settings(&self, settings: serde_json::Value);

    /// Writes and removals that failed since the last call, as messages a person can read.
    fn drain_failures(&self) -> Vec<String>;
}

/// A store that keeps the latest copy of each session in memory (tests). Loading is immediate.
#[derive(Debug, Default)]
pub struct MemoryStore {
    sessions: Mutex<Vec<Session>>,
    history: Mutex<Option<History>>,
    settings: Mutex<Option<serde_json::Value>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that already holds `sessions` (newest first), as if saved by an earlier launch.
    pub fn with_sessions(sessions: Vec<Session>) -> Self {
        MemoryStore { sessions: Mutex::new(sessions), ..Self::default() }
    }

    /// The settings saved last, if any.
    pub fn settings(&self) -> Option<serde_json::Value> {
        self.settings.lock().ok().and_then(|s| s.clone())
    }

    /// What is saved now, in the order sessions were first saved.
    pub fn sessions(&self) -> Vec<Session> {
        self.sessions.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl SessionStore for MemoryStore {
    fn load(&self, done: Notifier) {
        if let Ok(mut history) = self.history.lock() {
            *history = Some(History { sessions: self.sessions(), failures: Vec::new() });
        }
        done();
    }

    fn take_history(&self) -> Option<History> {
        self.history.lock().ok().and_then(|mut h| h.take())
    }

    fn save(&self, session: Session) {
        if let Ok(mut saved) = self.sessions.lock() {
            match saved.iter_mut().find(|s| s.id == session.id) {
                Some(slot) => *slot = session,
                None => saved.push(session),
            }
        }
    }

    fn delete(&self, id: SessionId) {
        if let Ok(mut saved) = self.sessions.lock() {
            saved.retain(|s| s.id != id);
        }
    }

    fn save_settings(&self, settings: serde_json::Value) {
        if let Ok(mut saved) = self.settings.lock() {
            *saved = Some(settings);
        }
    }

    fn drain_failures(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn memory_store_keeps_the_latest_copy_and_forgets_deleted_sessions() {
        let store = MemoryStore::new();
        store.save(Session::new(SessionId(1)));
        store.save(Session::with_title(SessionId(2), "two"));
        store.save(Session::with_title(SessionId(1), "renamed"));
        store.delete(SessionId(2));
        let saved = store.sessions();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].title, "renamed");
    }

    #[test]
    fn memory_store_hands_its_history_over_once_and_wakes_the_caller() {
        let store = MemoryStore::with_sessions(vec![Session::new(SessionId(5))]);
        assert_eq!(store.take_history(), None, "nothing before a load");
        let woke = Arc::new(AtomicBool::new(false));
        let flag = woke.clone();
        store.load(Arc::new(move || flag.store(true, Ordering::SeqCst)));
        assert!(woke.load(Ordering::SeqCst));
        assert_eq!(store.take_history().map(|h| h.sessions.len()), Some(1));
        assert_eq!(store.take_history(), None, "handed over once");
    }
}
