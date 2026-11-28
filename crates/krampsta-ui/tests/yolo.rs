#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The composer's yolo switch: it turns the mode on for the session shown, says so under the
//! field, and turns it off again.

use std::sync::Arc;

use krampsta_agent::{MockCatalog, MockConfig};
use krampsta_ui::components::composer::YOLO_HINT;
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

fn shows(rt: &Runtime<KrampstaApp>, needle: &str) -> bool {
    rt.elements().iter().any(|e| !e.exiting && e.text.as_deref().is_some_and(|t| t.contains(needle)))
}

#[test]
fn the_switch_turns_yolo_mode_on_and_off_for_the_session() {
    let mut state = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())));
    state.new_session();
    let id = state.sessions[0].id;
    let mut rt = Runtime::new(state, app::view, (1200.0, 800.0), Env::new(true, 1.0));
    settle(&mut rt);
    assert!(!rt.state.is_yolo(id) && !shows(&rt, YOLO_HINT), "off by default");

    click(&mut rt, keys::COMPOSER_YOLO);
    assert!(rt.state.is_yolo(id));
    assert!(shows(&rt, YOLO_HINT), "the composer warns while it is on");

    click(&mut rt, keys::COMPOSER_YOLO);
    assert!(!rt.state.is_yolo(id) && !shows(&rt, YOLO_HINT));
}
