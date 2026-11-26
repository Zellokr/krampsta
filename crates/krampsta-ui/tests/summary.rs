#![allow(clippy::unwrap_used, clippy::expect_used)]

//! A summary of earlier messages in the transcript: folded until clicked, then its text.

use std::sync::Arc;

use krampsta_agent::{MockCatalog, MockConfig};
use krampsta_core::MessageRole;
use krampsta_ui::{KrampstaApp, app, keys};
use rune::{Env, Runtime};

fn settle(rt: &mut Runtime<KrampstaApp>) {
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
}

fn shows(rt: &Runtime<KrampstaApp>, needle: &str) -> bool {
    rt.elements().iter().any(|e| !e.exiting && e.text.as_deref().is_some_and(|t| t.contains(needle)))
}

#[test]
fn a_summary_opens_and_closes_on_click() {
    let mut state = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())));
    state.new_session();
    let session = &mut state.sessions[0];
    session.push_message(MessageRole::User, "Profile the parser");
    let id = session.push_message(MessageRole::Summary, "The table is cached now.");
    session.push_message(MessageRole::User, "Add a benchmark");
    let mut rt = Runtime::new(state, app::view, (1200.0, 800.0), Env::new(true, 1.0));
    settle(&mut rt);

    assert!(shows(&rt, "Earlier messages summed up"));
    assert!(!shows(&rt, "The table is cached now."), "a summary starts folded");
    let header = keys::summary(id);
    let (x, y) = rt.find(&header).expect("the summary's header");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert!(shows(&rt, "The table is cached now."), "an open summary shows its text");

    let (x, y) = rt.find(&header).expect("the summary's header");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert!(!shows(&rt, "The table is cached now."));
}
