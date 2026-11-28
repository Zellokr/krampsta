//! Checkpoints: the files Krampsta wrote for a session, by the turn whose run proposed them, so
//! [`Session::rewind`] can put them back as they were and take the conversation back to before
//! that turn. Only Krampsta's own writes are kept (accepted changes, yolo mode): what a command
//! changed on disk is not, and stays.

use std::sync::Arc;

use crate::{MessageId, MessageRole, Session, SessionError};

/// A file Krampsta wrote for a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileWrite {
    /// The user message whose turn proposed the change.
    pub turn: MessageId,
    /// Relative to the project.
    pub path: String,
    /// What the file held before (`None`: it didn't exist).
    pub before: Option<Arc<str>>,
    /// What was written.
    pub after: Arc<str>,
}

/// One file to put back: it should still hold `expected` (what Krampsta last wrote), and gets
/// `content` (what it held before the turn; `None`: it didn't exist, so it goes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restore {
    pub path: String,
    pub expected: Arc<str>,
    pub content: Option<Arc<str>>,
}

/// What a rewind leaves to do: the message it took back, for the composer, and the files to
/// put back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewound {
    pub text: String,
    pub image_png: Option<Arc<str>>,
    pub restores: Vec<Restore>,
}

impl Session {
    /// Keeps a write Krampsta made for this session.
    pub fn record_write(&mut self, write: FileWrite) {
        self.writes.push(write);
    }

    pub fn writes(&self) -> &[FileWrite] {
        &self.writes
    }

    /// The files a rewind to `turn` would put back, one per path: what it held before the
    /// first write since, expecting what the last one wrote.
    pub fn restores_since(&self, turn: MessageId) -> Vec<Restore> {
        let mut restores: Vec<Restore> = Vec::new();
        for write in self.writes.iter().filter(|w| w.turn >= turn) {
            match restores.iter_mut().find(|r| r.path == write.path) {
                Some(restore) => restore.expected = write.after.clone(),
                None => restores.push(Restore {
                    path: write.path.clone(),
                    expected: write.after.clone(),
                    content: write.before.clone(),
                }),
            }
        }
        restores
    }

    /// Takes the conversation back to before the user message `turn`: it and everything after
    /// it go, and so do the writes made since, which come back as the files to put back.
    pub fn rewind(&mut self, turn: MessageId) -> Result<Rewound, SessionError> {
        if self.status.is_running() {
            return Err(SessionError::AlreadyRunning);
        }
        let at = self
            .messages
            .iter()
            .position(|m| m.id == turn && m.role == MessageRole::User)
            .ok_or(SessionError::NoSuchMessage)?;
        let restores = self.restores_since(turn);
        self.writes.retain(|w| w.turn < turn);
        let message = self.messages.swap_remove(at);
        self.messages.truncate(at);
        Ok(Rewound { text: message.content, image_png: message.image_png, restores })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentEvent, AgentStatus, RunId, SessionId};

    fn write(turn: u64, path: &str, before: Option<&str>, after: &str) -> FileWrite {
        FileWrite { turn: MessageId(turn), path: path.into(), before: before.map(Into::into), after: after.into() }
    }

    /// Three turns: the first wrote a.rs, the second a.rs again and created b.rs.
    fn worked() -> (Session, MessageId, MessageId) {
        let mut s = Session::new(SessionId(1));
        let mut turns = Vec::new();
        for (n, prompt) in ["one", "two", "three"].into_iter().enumerate() {
            let run = RunId(n as u64);
            turns.push(s.submit(prompt, run).unwrap());
            s.apply(&AgentEvent::Started { run });
            s.apply(&AgentEvent::TextDelta { run, text: format!("reply {n}") });
            s.apply(&AgentEvent::Completed { run });
        }
        s.record_write(write(turns[0].0, "a.rs", Some("v0"), "v1"));
        s.record_write(write(turns[1].0, "a.rs", Some("v1"), "v2"));
        s.record_write(write(turns[1].0, "b.rs", None, "new"));
        (s, turns[0], turns[1])
    }

    #[test]
    fn a_rewind_puts_back_each_file_once_and_takes_the_conversation_back() {
        let (mut s, first, second) = worked();
        let rewound = s.rewind(second).unwrap();
        assert_eq!((rewound.text.as_str(), rewound.image_png), ("two", None));
        assert_eq!(
            rewound.restores,
            [
                Restore { path: "a.rs".into(), expected: "v2".into(), content: Some("v1".into()) },
                Restore { path: "b.rs".into(), expected: "new".into(), content: None },
            ]
        );
        let left: Vec<&str> = s.messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(left, ["one", "reply 0"]);
        assert_eq!(s.writes(), [write(first.0, "a.rs", Some("v0"), "v1")], "the first turn's write stays");

        let rewound = s.rewind(first).unwrap();
        assert_eq!(rewound.restores[0].content.as_deref(), Some("v0"));
        assert!(s.messages.is_empty() && s.writes().is_empty());
    }

    #[test]
    fn a_rewind_further_back_puts_a_file_back_as_it_was_first() {
        let (s, first, _) = worked();
        let restores = s.restores_since(first);
        assert_eq!(restores[0], Restore { path: "a.rs".into(), expected: "v2".into(), content: Some("v0".into()) });
    }

    #[test]
    fn only_a_user_message_of_an_idle_session_can_be_rewound_to() {
        let (mut s, first, _) = worked();
        let reply = s.messages[1].id;
        assert_eq!(s.rewind(reply), Err(SessionError::NoSuchMessage));
        s.status = AgentStatus::Running;
        assert_eq!(s.rewind(first), Err(SessionError::AlreadyRunning));
        assert_eq!(s.messages.len(), 6, "nothing changed");
    }
}
