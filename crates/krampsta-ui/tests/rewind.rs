#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Rewinding from the confirmation dialog: the conversation goes back to before the message,
//! and its text returns to the composer.

use std::sync::Arc;

use krampsta_agent::{MockCatalog, MockConfig};
use krampsta_core::{AgentEvent, RunId};
use krampsta_ui::{KrampstaApp, app, keys};
use rune::{Env, Runtime};

fn settle(rt: &mut Runtime<KrampstaApp>) {
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
}

fn click(rt: &mut Runtime<KrampstaApp>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

#[test]
fn confirming_a_rewind_takes_the_conversation_back_and_cancelling_does_nothing() {
    let mut state = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())));
    state.new_session();
    let id = state.sessions[0].id;
    let mut turns = Vec::new();
    for (n, prompt) in ["first", "second"].into_iter().enumerate() {
        let x = &mut state.sessions[0];
        let run = RunId(n as u64 + 1);
        turns.push(x.submit(prompt, run).unwrap());
        x.apply(&AgentEvent::Started { run });
        x.apply(&AgentEvent::TextDelta { run, text: format!("reply {n}") });
        x.apply(&AgentEvent::Completed { run });
    }
    let mut rt = Runtime::new(state, app::view, (1200.0, 800.0), Env::new(true, 1.0));
    settle(&mut rt);

    rt.update(|s, _| s.ask_rewind(id, turns[1]));
    settle(&mut rt);
    click(&mut rt, keys::REWIND_CANCEL);
    assert_eq!(rt.state.sessions[0].messages.len(), 4, "cancelled: nothing changed");

    rt.update(|s, _| s.ask_rewind(id, turns[1]));
    settle(&mut rt);
    assert!(rt.find(keys::REWIND_CONFIRM).is_some(), "the confirmation shows");
    click(&mut rt, keys::REWIND_CONFIRM);
    let left: Vec<&str> = rt.state.sessions[0].messages.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(left, ["first", "reply 0"]);
    assert_eq!(rt.state.draft_of(id).text, "second", "the message is back in the composer");
    assert!(rt.find(keys::REWIND_CONFIRM).is_none(), "the dialog closes");
}
