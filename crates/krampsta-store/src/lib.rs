//! Krampsta's saved history.
//!
//! [`FileStore`] keeps each session as one JSON file, `<id>.json`, in a folder (by default
//! [`default_dir`], in the user's data folder). It implements
//! [`SessionStore`](krampsta_agent::SessionStore): the UI only queues work, and one writer
//! thread serializes and writes it, so no frame waits on the disk. Saves of the same session
//! that pile up before the writer gets to them collapse into one write of the newest copy.
//! Each file is written beside its final name and renamed over it, so a crash never leaves
//! half a session behind. The same thread reads the history when asked
//! ([`SessionStore::load`](krampsta_agent::SessionStore::load)), before any write queued after.
//!
//! This crate knows nothing about the UI framework.

mod format;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use krampsta_agent::{History, Notifier, SessionStore};
use krampsta_core::{Session, SessionId};

use crate::format::SavedSession;

/// Where the history lives unless told otherwise: `<data folder>/krampsta/sessions`.
pub fn default_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("krampsta").join("sessions"))
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not save the session to {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },
    #[error("could not save the settings to {path}: {source}")]
    Settings { path: PathBuf, source: std::io::Error },
    #[error("could not delete the saved session {path}: {source}")]
    Delete { path: PathBuf, source: std::io::Error },
    #[error("could not read the saved session {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("the saved session {path} is damaged: {source}")]
    Damaged { path: PathBuf, source: serde_json::Error },
    #[error("the saved session {path} comes from a newer Krampsta (format {found}, this one reads {known})")]
    Newer { path: PathBuf, found: u32, known: u32 },
    #[error("could not start the history writer: {0}")]
    Thread(std::io::Error),
}

/// What [`load`] found: the sessions it could read, newest first, and why the others couldn't be.
#[derive(Debug, Default)]
pub struct Loaded {
    pub sessions: Vec<Session>,
    pub failures: Vec<StoreError>,
}

/// Reads every session saved in `dir`. Blocking: call it off the UI thread. A missing folder
/// is an empty history.
pub fn load(dir: &Path) -> Loaded {
    let mut loaded = Loaded::default();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return loaded,
        Err(source) => {
            loaded.failures.push(StoreError::Read { path: dir.to_owned(), source });
            return loaded;
        }
    };
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(source) => {
                loaded.failures.push(StoreError::Read { path: dir.to_owned(), source });
                continue;
            }
        };
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        match read_session(&path) {
            Ok(session) => loaded.sessions.push(session),
            Err(e) => loaded.failures.push(e),
        }
    }
    // Ids grow with time (see `krampsta_core::IdGen::from_clock`), so this is newest first.
    loaded.sessions.sort_by_key(|s| std::cmp::Reverse(s.id));
    loaded
}

fn read_session(path: &Path) -> Result<Session, StoreError> {
    let text = fs::read_to_string(path).map_err(|source| StoreError::Read { path: path.to_owned(), source })?;
    let saved: SavedSession =
        serde_json::from_str(&text).map_err(|source| StoreError::Damaged { path: path.to_owned(), source })?;
    if saved.version > format::VERSION {
        return Err(StoreError::Newer { path: path.to_owned(), found: saved.version, known: format::VERSION });
    }
    Ok(saved.into_session())
}

/// The file a session is saved in.
fn file_for(dir: &Path, id: SessionId) -> PathBuf {
    dir.join(format!("{id}.json"))
}

enum Op {
    Save(Box<Session>),
    Delete,
}

#[derive(Default)]
struct Queue {
    pending: HashMap<SessionId, Op>,
    /// The newest settings not yet written, and the file they go to.
    settings: Option<(PathBuf, serde_json::Value)>,
    failures: Vec<String>,
    /// A load was asked for: whom to wake once `history` holds it.
    load: Option<Notifier>,
    history: Option<History>,
    /// The store is being dropped: write what is left, then stop.
    closing: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        // A panic while holding the lock leaves the queue as it was between two operations,
        // so it stays usable.
        self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Sessions saved as JSON files in one folder, written by a background thread.
pub struct FileStore {
    dir: PathBuf,
    /// Where the settings go. None: they can't be saved (and saying so is a failure).
    settings_file: Option<PathBuf>,
    shared: Arc<Shared>,
    writer: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for FileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStore").field("dir", &self.dir).finish_non_exhaustive()
    }
}

impl FileStore {
    /// A store writing into `dir`, created on the first write. Touches no files itself.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let dir = dir.into();
        let shared = Arc::new(Shared::default());
        let writer = {
            let (dir, shared) = (dir.clone(), shared.clone());
            std::thread::Builder::new()
                .name("krampsta-store".into())
                .spawn(move || write_loop(&dir, &shared))
                .map_err(StoreError::Thread)?
        };
        Ok(FileStore { dir, settings_file: None, shared, writer: Some(writer) })
    }

    /// Saves the settings to `file` (see [`load_settings`]).
    pub fn with_settings_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.settings_file = Some(file.into());
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn queue(&self, id: SessionId, op: Op) {
        self.shared.lock().pending.insert(id, op);
        self.shared.wake.notify_one();
    }
}

impl SessionStore for FileStore {
    fn load(&self, done: Notifier) {
        self.shared.lock().load = Some(done);
        self.shared.wake.notify_one();
    }

    fn take_history(&self) -> Option<History> {
        self.shared.lock().history.take()
    }

    fn save(&self, session: Session) {
        self.queue(session.id, Op::Save(Box::new(session)));
    }

    fn delete(&self, id: SessionId) {
        self.queue(id, Op::Delete);
    }

    fn save_settings(&self, settings: serde_json::Value) {
        let mut queue = self.shared.lock();
        match &self.settings_file {
            Some(file) => queue.settings = Some((file.clone(), settings)),
            None => queue.failures.push("the settings have nowhere to be saved".into()),
        }
        drop(queue);
        self.shared.wake.notify_one();
    }

    fn drain_failures(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.lock().failures)
    }
}

impl Drop for FileStore {
    /// Writes whatever is still queued before the app goes away.
    fn drop(&mut self) {
        self.shared.lock().closing = true;
        self.shared.wake.notify_one();
        if let Some(writer) = self.writer.take()
            && writer.join().is_err()
        {
            eprintln!("krampsta: the history writer stopped unexpectedly; recent changes may not be saved");
        }
    }
}

fn write_loop(dir: &Path, shared: &Shared) {
    loop {
        let (load_for, batch, settings, closing) = {
            let mut queue = shared.lock();
            while queue.pending.is_empty() && queue.settings.is_none() && queue.load.is_none() && !queue.closing {
                queue = shared.wake.wait(queue).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            (queue.load.take(), std::mem::take(&mut queue.pending), queue.settings.take(), queue.closing)
        };
        if let Some(done) = load_for {
            let loaded = load(dir);
            let failures = loaded.failures.iter().map(ToString::to_string).collect();
            shared.lock().history = Some(History { sessions: loaded.sessions, failures });
            done();
        }
        let settings = settings.and_then(|(path, value)| {
            write_atomically(&path, &value).err().map(|source| StoreError::Settings { path, source })
        });
        let failures: Vec<String> = batch
            .into_iter()
            .filter_map(|(id, op)| apply(dir, id, op).err())
            .chain(settings)
            .map(|e| {
                let message = e.to_string();
                if closing {
                    // Nobody is left to show it.
                    eprintln!("krampsta: {message}");
                }
                message
            })
            .collect();
        if !failures.is_empty() {
            shared.lock().failures.extend(failures);
        }
        if closing && shared.lock().pending.is_empty() && shared.lock().settings.is_none() {
            return;
        }
    }
}

fn apply(dir: &Path, id: SessionId, op: Op) -> Result<(), StoreError> {
    let path = file_for(dir, id);
    match op {
        Op::Save(session) => write_atomically(&path, &SavedSession::from_session(&session))
            .map_err(|source| StoreError::Write { path, source }),
        Op::Delete => match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            // Never saved (an empty session), or already gone: nothing to do.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StoreError::Delete { path, source }),
        },
    }
}

/// Writes `value` as JSON beside `path`, then renames it over `path`, creating its folder.
fn write_atomically(path: &Path, value: &impl serde::Serialize) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, json).and_then(|()| fs::rename(&temp, path)).inspect_err(|_| {
        // Best effort: a leftover temporary file is harmless (loading skips it).
        let _ = fs::remove_file(&temp);
    })
}

/// Where the settings live unless told otherwise: `<config folder>/krampsta/settings.json`,
/// beside the permission rules.
pub fn default_settings_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("krampsta").join("settings.json"))
}

/// Reads the saved settings. Blocking, but one small file: the app reads it before its first
/// frame so the window opens as it was left. None when nothing was saved yet.
pub fn load_settings(file: &Path) -> Result<Option<serde_json::Value>, StoreError> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(StoreError::Read { path: file.to_owned(), source }),
    };
    serde_json::from_str(&text).map(Some).map_err(|source| StoreError::Damaged { path: file.to_owned(), source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use krampsta_core::{AgentEvent, AgentStatus, MessageRole, RunId, ToolCall, ToolCallId, ToolResult, Usage};

    /// A session that went through a whole run with a tool call, reasoning and usage.
    fn worked_session(id: u64) -> Session {
        let mut s = Session::new(SessionId(id));
        s.project = Some(PathBuf::from("/code/krampsta"));
        let run = RunId(3);
        s.submit("List the files", run).unwrap();
        s.bill_to("openai", "gpt-test");
        s.apply(&AgentEvent::Started { run });
        s.apply(&AgentEvent::ReasoningDelta { run, text: "Looking".into() });
        let call = ToolCall { id: ToolCallId("c1".into()), name: "list_files".into(), arguments: "{}".into() };
        s.apply(&AgentEvent::ToolCallStarted { run, call: call.clone() });
        s.apply(&AgentEvent::ToolCallFinished {
            run,
            result: ToolResult { call_id: call.id, output: "src/\nCargo.toml".into(), is_error: false },
        });
        s.apply(&AgentEvent::TextDelta { run, text: "Two entries: `src/` and `Cargo.toml`.".into() });
        s.apply(&AgentEvent::Usage { run, usage: Usage { input: 120, output: 30 } });
        s.apply(&AgentEvent::Completed { run });
        s
    }

    /// Dropping the store waits for the writer to finish what was queued.
    fn save_all(dir: &Path, sessions: &[Session]) {
        let store = FileStore::open(dir).unwrap();
        for s in sessions {
            store.save(s.clone());
        }
        drop(store);
    }

    #[test]
    fn saved_sessions_load_back_as_they_were() {
        let dir = tempfile::tempdir().unwrap();
        let session = worked_session(7);
        save_all(dir.path(), std::slice::from_ref(&session));

        let loaded = load(dir.path());
        assert!(loaded.failures.is_empty(), "{:?}", loaded.failures);
        assert_eq!(loaded.sessions, vec![session]);
    }

    #[test]
    fn a_session_saved_mid_run_comes_back_cancelled_without_its_live_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Session::new(SessionId(1));
        let run = RunId(1);
        s.submit("build", run).unwrap();
        let call = ToolCall { id: ToolCallId("c".into()), name: "run_command".into(), arguments: "{}".into() };
        s.apply(&AgentEvent::ToolCallStarted { run, call: call.clone() });
        s.apply(&AgentEvent::ToolOutputDelta { run, call: call.id.clone(), text: "Compiling\n".into() });
        save_all(dir.path(), &[s]);

        let restored = load(dir.path()).sessions.remove(0);
        assert_eq!(restored.status, AgentStatus::Cancelled);
        assert_eq!(restored.messages[1].live_output_for(&call), None);
        assert_eq!(restored.messages[1].tool_calls, vec![call]);
    }

    #[test]
    fn later_saves_win_and_deletes_remove_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::open(dir.path()).unwrap();
        let mut s = Session::new(SessionId(1));
        s.push_message(MessageRole::User, "first");
        store.save(s.clone());
        s.push_message(MessageRole::User, "second");
        store.save(s.clone());
        store.save(worked_session(2));
        store.save(worked_session(3));
        store.delete(SessionId(3));
        drop(store);

        let loaded = load(dir.path());
        let ids: Vec<SessionId> = loaded.sessions.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![SessionId(2), SessionId(1)], "newest first, the deleted one gone");
        assert_eq!(loaded.sessions[1].messages.len(), 2);
        assert!(!file_for(dir.path(), SessionId(3)).exists());
        let leftovers = fs::read_dir(dir.path()).unwrap().filter_map(Result::ok);
        assert!(leftovers.into_iter().all(|e| e.path().extension().is_some_and(|x| x == "json")), "no temporary files");
    }

    #[test]
    fn deleting_a_session_that_was_never_saved_is_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::open(dir.path()).unwrap();
        store.delete(SessionId(42));
        drop(store);
        assert!(load(dir.path()).failures.is_empty());
    }

    #[test]
    fn write_failures_are_reported_not_swallowed() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the sessions folder should be: nothing can be written inside it.
        let blocked = dir.path().join("sessions");
        fs::write(&blocked, "not a folder").unwrap();
        let store = FileStore::open(&blocked).unwrap();
        store.save(worked_session(1));
        let failures = (0..200)
            .find_map(|_| {
                let failures = store.drain_failures();
                if failures.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    None
                } else {
                    Some(failures)
                }
            })
            .expect("the writer reports the failed save within two seconds");
        assert!(failures[0].contains("could not save the session"), "{failures:?}");
    }

    #[test]
    fn damaged_and_newer_files_are_reported_and_the_rest_still_load() {
        let dir = tempfile::tempdir().unwrap();
        save_all(dir.path(), &[worked_session(1)]);
        fs::write(dir.path().join("ses2.json"), "{ not json").unwrap();
        let newer = r#"{"version": 99, "id": 3, "title": "t", "status": {"kind": "idle"}, "messages": []}"#;
        // Saved before sessions had a project: it loads with none.
        let older = r#"{"version": 1, "id": 4, "title": "t", "status": {"kind": "idle"}, "messages": []}"#;
        fs::write(dir.path().join("ses4.json"), older).unwrap();
        fs::write(dir.path().join("ses3.json"), newer).unwrap();
        fs::write(dir.path().join("notes.txt"), "ignored").unwrap();

        let loaded = load(dir.path());
        assert_eq!(loaded.sessions.len(), 2);
        assert_eq!(loaded.sessions[0].project, None, "the older file");
        assert_eq!(loaded.failures.len(), 2);
        assert!(loaded.failures.iter().any(|e| matches!(e, StoreError::Damaged { .. })));
        assert!(loaded.failures.iter().any(|e| matches!(e, StoreError::Newer { found: 99, .. })));
    }

    #[test]
    fn load_reads_in_the_background_and_wakes_the_caller_once_it_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        save_all(dir.path(), &[worked_session(1), worked_session(2)]);
        fs::write(dir.path().join("ses3.json"), "{ not json").unwrap();

        let store = FileStore::open(dir.path()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        store.load(Arc::new(move || {
            let _ = tx.lock().map(|tx| tx.send(()));
        }));
        rx.recv_timeout(std::time::Duration::from_secs(5)).expect("load wakes the caller");
        let history = store.take_history().expect("the history is ready once woken");
        let ids: Vec<SessionId> = history.sessions.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![SessionId(2), SessionId(1)]);
        assert_eq!(history.failures.len(), 1);
        assert!(history.failures[0].contains("ses3.json"), "{:?}", history.failures);
        assert!(store.take_history().is_none(), "handed over once");
    }

    #[test]
    fn settings_save_beside_nothing_else_and_load_back_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config").join("settings.json");
        assert!(load_settings(&file).unwrap().is_none(), "nothing saved yet");
        let store = FileStore::open(dir.path().join("sessions")).unwrap().with_settings_file(&file);
        store.save_settings(serde_json::json!({ "theme": "dark" }));
        store.save_settings(serde_json::json!({ "theme": "light" }));
        drop(store);
        assert_eq!(load_settings(&file).unwrap(), Some(serde_json::json!({ "theme": "light" })));
        assert!(load(&dir.path().join("sessions")).sessions.is_empty(), "no session files");
    }

    #[test]
    fn damaged_settings_and_a_store_without_a_settings_file_say_so() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(&file, "{ nope").unwrap();
        assert!(matches!(load_settings(&file), Err(StoreError::Damaged { .. })));

        let store = FileStore::open(dir.path()).unwrap();
        store.save_settings(serde_json::json!({}));
        assert_eq!(store.drain_failures(), vec!["the settings have nowhere to be saved".to_owned()]);
    }

    #[test]
    fn a_missing_folder_is_an_empty_history() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(&dir.path().join("nothing-here"));
        assert!(loaded.sessions.is_empty() && loaded.failures.is_empty());
    }
}
