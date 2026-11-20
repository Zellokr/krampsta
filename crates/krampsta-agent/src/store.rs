//! Where sessions are kept between launches.
//!
//! The UI calls a [`SessionStore`] from its own thread, so every method returns at once: an
//! implementation queues the work and does the I/O elsewhere. What fails comes back later
//! through [`SessionStore::drain_failures`].

use std::sync::Mutex;

use krampsta_core::{Session, SessionId};

/// Keeps sessions somewhere durable. Never blocks the caller.
pub trait SessionStore: Send + Sync {
    /// Queues `session` to be written, replacing what was saved for it. Saving the same session
    /// again before the write happens only writes the newest copy.
    fn save(&self, session: Session);

    /// Queues the session's removal.
    fn delete(&self, id: SessionId);

    /// Writes and removals that failed since the last call, as messages a person can read.
    fn drain_failures(&self) -> Vec<String>;
}

/// A store that keeps the latest copy of each session in memory (tests).
#[derive(Debug, Default)]
pub struct MemoryStore {
    sessions: Mutex<Vec<Session>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// What is saved now, in the order sessions were first saved.
    pub fn sessions(&self) -> Vec<Session> {
        self.sessions.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl SessionStore for MemoryStore {
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

    fn drain_failures(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
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
}
