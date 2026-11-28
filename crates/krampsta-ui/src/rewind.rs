//! Rewinding a session to before one of the user's messages (its menu's "Rewind to here…",
//! after a confirmation): the conversation from that message on goes, its text comes back to the
//! composer, the changes those turns proposed and nobody accepted are rejected, and the files
//! Krampsta wrote for them are put back as they were (`Session::rewind`, through the host's
//! `restore_files`). What a command changed on disk is not undone; a file changed since
//! Krampsta wrote it is left as it is, and the composer says so.

use krampsta_core::{MessageId, SessionId};
use rune::TextEdit;

use crate::menus::Confirm;
use crate::state::KrampstaApp;

impl KrampstaApp {
    /// Opens the confirmation for rewinding `session` to before `message`.
    pub fn ask_rewind(&mut self, session: SessionId, message: MessageId) {
        self.menu = None;
        self.confirm = Some(Confirm::Rewind { session, message });
    }

    /// How many files a rewind of `session` to `message` would put back.
    pub fn rewind_files(&self, session: SessionId, message: MessageId) -> usize {
        self.session(session).map_or(0, |x| x.restores_since(message).len())
    }

    /// Rewinds the session the confirmation asks about.
    pub fn confirm_rewind(&mut self) {
        let Some(Confirm::Rewind { session, message }) = self.confirm else { return };
        self.confirm = None;
        let Some(x) = self.session_mut(session) else { return };
        let rewound = match x.rewind(message) {
            Ok(rewound) => rewound,
            Err(e) => {
                self.notice = Some(format!("Can't rewind: {e}."));
                return;
            }
        };
        self.drop_proposals_since(session, message);
        if !rewound.restores.is_empty()
            && let Err(e) = self.agent.restore_files(session, rewound.restores)
        {
            self.notice = Some(format!("The files couldn't be put back: {e}"));
        }
        self.view_mut(session).draft = TextEdit::new(&rewound.text);
        self.focus_composer(session);
        self.persist(session);
        self.sessions_changed();
    }

    /// Takes the answers to rewinds' file restores; a file that couldn't go back is said in the
    /// composer. True when one came.
    pub(crate) fn land_restores(&mut self) -> bool {
        let answers = self.agent.drain_restores();
        for done in &answers {
            if done.restored > 0 {
                self.disk_changed();
            }
            if !done.failures.is_empty() {
                let n = done.failures.len();
                let files = if n == 1 { "1 file" } else { "files" };
                self.notice = Some(format!("{files} couldn't be put back: {}", done.failures.join("; ")));
            }
        }
        !answers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use krampsta_agent::{MockCatalog, ToolOutput, ToolSpec, Toolbox};
    use krampsta_core::{AgentEvent, FileChange, FileWrite, RunId};

    use super::*;

    /// A disk in memory that can write and put back.
    #[derive(Default)]
    struct Disk {
        files: Mutex<HashMap<String, String>>,
    }

    impl Toolbox for Disk {
        fn specs(&self) -> Arc<[ToolSpec]> {
            Arc::from([])
        }
        fn run(&self, _: &str, _: &str, _: &krampsta_agent::Call<'_>) -> Result<ToolOutput, String> {
            Err("no tools".into())
        }
        fn restore(&self, path: &str, expected: &str, content: Option<&str>) -> Result<(), String> {
            let mut files = self.files.lock().unwrap();
            if files.get(path).map(String::as_str) != Some(expected) {
                return Err(format!("`{path}` changed since Krampsta wrote it"));
            }
            match content {
                Some(c) => files.insert(path.into(), c.into()),
                None => files.remove(path),
            };
            Ok(())
        }
    }

    /// A session with two finished turns; the second wrote a.rs and created b.rs, and proposed
    /// c.rs (not accepted).
    fn rewound_app(disk: &Arc<Disk>) -> (KrampstaApp, SessionId, MessageId) {
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::default())).with_tools(disk.clone());
        app.new_session();
        let id = app.sessions[0].id;
        let mut second = MessageId(0);
        for (n, prompt) in ["one", "two"].into_iter().enumerate() {
            let x = &mut app.sessions[0];
            let run = RunId(n as u64 + 1);
            second = x.submit(prompt, run).unwrap();
            x.apply(&AgentEvent::Started { run });
            x.apply(&AgentEvent::Completed { run });
        }
        let x = &mut app.sessions[0];
        x.record_write(FileWrite { turn: second, path: "a.rs".into(), before: Some("v1".into()), after: "v2".into() });
        x.record_write(FileWrite { turn: second, path: "b.rs".into(), before: None, after: "new".into() });
        disk.files.lock().unwrap().extend([("a.rs".into(), "v2".into()), ("b.rs".into(), "new".into())]);
        let proposal = FileChange { path: "c.rs".into(), before: None, after: "later".into() };
        app.stage_proposal_from(id, proposal);
        (app, id, second)
    }

    fn pump_until(app: &mut KrampstaApp, done: impl Fn(&KrampstaApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(app) {
            assert!(Instant::now() < deadline, "never happened");
            app.pump();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_rewind_puts_files_back_and_returns_the_message_to_the_composer() {
        let disk = Arc::new(Disk::default());
        let (mut app, id, second) = rewound_app(&disk);
        assert_eq!(app.rewind_files(id, second), 2);

        app.ask_rewind(id, second);
        assert_eq!(app.confirm, Some(Confirm::Rewind { session: id, message: second }));
        app.confirm_rewind();
        assert_eq!(app.confirm, None);
        let left: Vec<&str> = app.sessions[0].messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(left, ["one"], "the second turn is gone (its empty reply was never kept)");
        assert_eq!(app.draft_of(id).text, "two", "its text is back in the composer");
        assert!(app.review.proposals().is_empty(), "what it proposed and nobody accepted goes");

        pump_until(&mut app, |_| disk.files.lock().unwrap().get("a.rs").map(String::as_str) == Some("v1"));
        assert!(!disk.files.lock().unwrap().contains_key("b.rs"), "a file Krampsta created goes");
        assert_eq!(app.notice, None);
    }

    #[test]
    fn a_file_changed_since_is_left_and_said() {
        let disk = Arc::new(Disk::default());
        let (mut app, id, second) = rewound_app(&disk);
        disk.files.lock().unwrap().insert("a.rs".into(), "edited by hand".into());
        app.ask_rewind(id, second);
        app.confirm_rewind();
        pump_until(&mut app, |a| a.notice.is_some());
        assert!(app.notice.as_deref().unwrap().contains("changed since Krampsta wrote it"));
        assert_eq!(disk.files.lock().unwrap()["a.rs"], "edited by hand");
        assert!(!disk.files.lock().unwrap().contains_key("b.rs"), "the others still go back");
    }
}
