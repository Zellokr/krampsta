#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The sessions in the sidebar, used as a person would: renamed with a double click on the row,
//! a name typed, Return to keep it or Escape to drop it; and searched from the field above them.

use std::sync::Arc;

use krampsta_agent::{MockCatalog, MockConfig};
use krampsta_ui::{KrampstaApp, app, keys};
use rune::{Env, Ime, Key, KeyEvent, Mods, Runtime};

fn runtime() -> Runtime<KrampstaApp> {
    let state = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant()))).with_sample_sessions();
    let mut rt = Runtime::new(state, app::view, (1200.0, 800.0), Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<KrampstaApp>) {
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
}

fn double_click(rt: &mut Runtime<KrampstaApp>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

/// Selects everything in the focused field and types `text` over it.
fn retype(rt: &mut Runtime<KrampstaApp>, text: &str) {
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    rt.ime(Ime::Commit(text.into()));
    rt.frame(1.0 / 60.0);
}

fn press(rt: &mut Runtime<KrampstaApp>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    settle(rt);
}

#[test]
fn a_double_click_renames_a_session_and_return_keeps_the_name() {
    let mut rt = runtime();
    let id = rt.state.sessions[1].id;
    let row = keys::session(id);
    double_click(&mut rt, &row);
    // rune-kit keys the field "rename"; only the row being renamed has one.
    let field = "rename";
    assert!(rt.find(field).is_some(), "the title turned into a field");

    retype(&mut rt, "Retry backoff, take two");
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.session(id).unwrap().title, "Retry backoff, take two");
    assert!(rt.find(field).is_none(), "the field closed");
}

#[test]
fn escape_keeps_the_old_name() {
    let mut rt = runtime();
    let id = rt.state.sessions[1].id;
    let old = rt.state.session(id).unwrap().title.clone();
    double_click(&mut rt, &keys::session(id));
    assert!(rt.find("rename").is_some(), "the field opened");
    retype(&mut rt, "Not this one");
    press(&mut rt, Key::Escape);
    assert_eq!(rt.state.session(id).unwrap().title, old);
    assert!(rt.find("rename").is_none());
}

#[test]
fn rename_in_the_rows_menu_opens_the_field_too() {
    let mut rt = runtime();
    let id = rt.state.sessions[0].id;
    let (x, y) = rt.find(&keys::session(id)).unwrap();
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    settle(&mut rt);
    // Items: Copy title, Rename, a separator, Delete….
    let (x, y) = rt.find(&format!("{}-item1", keys::session_menu(id))).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert!(rt.find("rename").is_some(), "the title turned into a field");
    retype(&mut rt, "From the menu");
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.session(id).unwrap().title, "From the menu");
}

fn listed(rt: &Runtime<KrampstaApp>, id: krampsta_core::SessionId) -> bool {
    rt.find(&keys::session(id)).is_some()
}

#[test]
fn the_search_shortcut_and_typing_filter_the_list_and_escape_brings_it_back() {
    let mut rt = runtime();
    let (welcome, retry) = (rt.state.sessions[0].id, rt.state.sessions[1].id);
    // App shortcuts go through `app::on_key`, as the window's spec routes them.
    let search = KeyEvent { key: Key::Command('f'), mods: Mods { cmd: true, shift: true, ..Mods::default() } };
    rt.update(|s, tx| {
        app::on_key(s, &search, tx);
    });
    settle(&mut rt);
    assert!(rt.state.search.query.focused, "Ctrl+Shift+F puts the caret in the search");

    rt.ime(Ime::Commit("jitter".into()));
    settle(&mut rt);
    assert!(listed(&rt, retry), "a session whose messages mention it");
    assert!(!listed(&rt, welcome), "one that doesn't is hidden");

    rt.ime(Ime::Commit(" nothing like this".into()));
    settle(&mut rt);
    assert!(rt.find(keys::SESSION_SEARCH_EMPTY).is_some(), "nothing matches: it says so");

    press(&mut rt, Key::Escape);
    assert!(listed(&rt, welcome) && listed(&rt, retry), "Escape empties the search");
    assert!(rt.find(keys::SESSION_SEARCH_EMPTY).is_none());
}
