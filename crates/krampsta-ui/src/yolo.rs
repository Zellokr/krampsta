//! Yolo mode, per session (`Session::yolo`, saved with it): what would ask the user is allowed
//! at once, answered as [`Decision::Yolo`] so the command log says so, and the changes the
//! session proposes are written without review (`changes.rs`, `write_auto`). Rules that deny
//! never ask, so they still deny. The composer shows the switch and, while on, a warning.

use krampsta_agent::Decision;
use krampsta_core::SessionId;

use crate::approvals::Asking;
use crate::state::KrampstaApp;

impl KrampstaApp {
    pub fn is_yolo(&self, session: SessionId) -> bool {
        self.session(session).is_some_and(|s| s.yolo)
    }

    /// Turns yolo mode on or off for `session`. On, it also allows the calls its run is
    /// already waiting on.
    pub fn toggle_yolo(&mut self, session: SessionId) {
        let Some(s) = self.session_mut(session) else { return };
        s.yolo = !s.yolo;
        let (on, run) = (s.yolo, s.active_run());
        if let (true, Some(run)) = (on, run) {
            let agent = &self.agent;
            self.approvals.retain(|a| {
                let waiting = a.run == run;
                if waiting {
                    agent.answer(a.run, &a.call, Decision::Yolo);
                }
                !waiting
            });
        }
        self.persist(session);
    }

    /// A call of `session`'s run asks to be allowed: yolo mode allows it, otherwise the user
    /// is asked (and called back if the window is behind).
    pub(crate) fn ask_from(&mut self, session: SessionId, asking: Asking) {
        if self.is_yolo(session) {
            // False only when the run already ended: nothing waits.
            self.agent.answer(asking.run, &asking.call, Decision::Yolo);
        } else {
            self.ask(asking);
            self.needs_attention(session);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::MockCatalog;
    use krampsta_core::{RunId, ToolCallId};

    use super::*;

    fn asking(run: RunId) -> Asking {
        Asking { run, call: ToolCallId("c".into()), tool: "run_command".into(), what: "cargo test".into() }
    }

    #[test]
    fn a_yolo_session_is_never_asked_and_others_are() {
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::default()));
        app.new_session();
        let id = app.sessions[0].id;
        app.ask_from(id, asking(RunId(1)));
        assert_eq!(app.approvals.len(), 1, "off: the user decides");

        app.approvals.clear();
        app.sessions[0].yolo = true;
        app.ask_from(id, asking(RunId(1)));
        assert!(app.approvals.is_empty(), "on: allowed without a dialog");
    }

    #[test]
    fn turning_it_on_allows_what_the_run_already_waits_on() {
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::default()));
        app.new_session();
        let id = app.sessions[0].id;
        app.sessions[0].submit("build it", RunId(7)).unwrap();
        app.ask(asking(RunId(7)));
        app.ask(asking(RunId(8)));

        app.toggle_yolo(id);
        assert!(app.is_yolo(id));
        assert_eq!(app.approvals.iter().map(|a| a.run).collect::<Vec<_>>(), [RunId(8)], "another run's stays");
        app.toggle_yolo(id);
        assert!(!app.is_yolo(id));
    }
}
