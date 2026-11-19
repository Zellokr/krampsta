//! The review chain as the app wires it: a real edit from the tools, accepted from the UI,
//! written by the host through the same tools, against a temporary project.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use krampsta_agent::{Approval, Call, CancellationToken, MockCatalog, Toolbox};
use krampsta_tools::Workspace;
use krampsta_ui::KrampstaApp;

fn edit(tools: &Workspace, args: &str) -> krampsta_core::FileChange {
    let cancel = CancellationToken::new();
    let call = Call { cancel: &cancel, approval: &Approval::NotNeeded, output: &|_| {} };
    tools.run("edit_file", args, &call).unwrap().change.unwrap()
}

fn pump_until(app: &mut KrampstaApp, done: impl Fn(&KrampstaApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(app) {
        assert!(Instant::now() < deadline, "timed out");
        app.pump();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn accepting_after_the_file_changed_on_disk_warns_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hola.txt"), "Hola\n").unwrap();
    let tools = Arc::new(Workspace::open_with(dir.path(), Some(config.path().to_owned())).unwrap());
    let mut app = KrampstaApp::new(Arc::new(MockCatalog::default())).with_tools(tools.clone());

    app.propose(edit(&tools, r#"{"path":"hola.txt","old_string":"Hola","new_string":"Adiós"}"#));
    // The user edits the file by hand before accepting.
    std::fs::write(dir.path().join("hola.txt"), "Hola, mundo\n").unwrap();
    app.accept_file(0);
    pump_until(&mut app, |a| a.review.error.is_some() || a.review.proposals().is_empty());

    assert!(app.review.error.as_deref().is_some_and(|e| e.contains("changed on disk")), "{:?}", app.review.error);
    assert_eq!(std::fs::read_to_string(dir.path().join("hola.txt")).unwrap(), "Hola, mundo\n", "their edit stays");
    assert_eq!(app.review.proposals().len(), 1, "the proposal waits to be rejected");
}
