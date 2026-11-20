//! History across launches, as the app wires it: a conversation with the mock agent saved by the
//! real `FileStore` into a temporary folder, then a second app over the same folder that reads
//! it back in the background and goes on with it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use krampsta_agent::{MockCatalog, MockConfig};
use krampsta_core::{AgentStatus, SessionId};
use krampsta_store::FileStore;
use krampsta_ui::KrampstaApp;
use rune::TextEdit;

fn launch(dir: &std::path::Path) -> KrampstaApp {
    let catalog = Arc::new(MockCatalog::new(MockConfig::instant()));
    KrampstaApp::new(catalog).with_store(Arc::new(FileStore::open(dir).unwrap()))
}

fn pump_until(app: &mut KrampstaApp, what: &str, done: impl Fn(&KrampstaApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(app) {
        assert!(Instant::now() < deadline, "{what} never happened");
        app.pump();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_conversation_saved_in_one_launch_comes_back_in_the_next() {
    let dir = tempfile::tempdir().unwrap();

    let mut first = launch(dir.path());
    let id: SessionId = first.new_session();
    first.draft = TextEdit::new("Explain the borrow checker");
    first.send().unwrap();
    pump_until(&mut first, "the reply", |a| a.session(id).is_some_and(|s| s.status == AgentStatus::Completed));
    let said = first.session(id).cloned().unwrap();
    // Closing the window drops the app, and the store writes what is still queued.
    drop(first);

    let mut second = launch(dir.path());
    assert!(second.session(id).is_none(), "nothing is read until asked");
    let woke = Arc::new(AtomicBool::new(false));
    let flag = woke.clone();
    second.load_history(Arc::new(move || flag.store(true, Ordering::SeqCst)));
    pump_until(&mut second, "the history landing", |a| a.session(id).is_some());
    assert!(woke.load(Ordering::SeqCst), "the store woke the window");
    assert_eq!(second.session(id), Some(&said), "the conversation, as it was");
    assert!(second.notice.is_none(), "nothing failed: {:?}", second.notice);

    second.select(id);
    second.draft = TextEdit::new("And lifetimes?");
    second.send().unwrap();
    pump_until(&mut second, "the second reply", |a| a.session(id).is_some_and(|s| s.status == AgentStatus::Completed));
    assert_eq!(second.session(id).unwrap().messages.len(), 4);
    drop(second);

    let reread = krampsta_store::load(dir.path());
    assert!(reread.failures.is_empty(), "{:?}", reread.failures);
    let saved = reread.sessions.iter().find(|s| s.id == id).unwrap();
    assert_eq!(saved.messages.len(), 4, "the continued conversation was saved too");
}
