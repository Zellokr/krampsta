#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The command palette, used as a person would: Ctrl+K, a few letters, Return (or a click on a
//! row), and Escape to close it.

use std::sync::Arc;

use krampsta_agent::{MockCatalog, MockConfig};
use krampsta_ui::files::Inspector;
use krampsta_ui::{KrampstaApp, ThemeChoice, app, keys};
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

/// App shortcuts go through `app::on_key`, as the window's spec routes them.
fn shortcut(rt: &mut Runtime<KrampstaApp>, c: char) {
    let ev = KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } };
    rt.update(|s, tx| {
        app::on_key(s, &ev, tx);
    });
    settle(rt);
}

fn press(rt: &mut Runtime<KrampstaApp>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    settle(rt);
}

#[test]
fn ctrl_k_finds_a_command_and_return_runs_it() {
    let mut rt = runtime();
    shortcut(&mut rt, 'k');
    assert!(rt.find(keys::PALETTE_INPUT).is_some(), "Ctrl+K opens the palette");

    rt.ime(Ime::Commit("files".into()));
    settle(&mut rt);
    assert_eq!(rt.state.palette.entries[0].label, "Show the project's files");
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.inspector, Inspector::Files);
    assert!(rt.find(keys::PALETTE_INPUT).is_none(), "running closes it");
}

#[test]
fn a_click_on_a_row_runs_it_and_escape_closes() {
    let mut rt = runtime();
    shortcut(&mut rt, 'k');
    rt.ime(Ime::Commit("theme light".into()));
    settle(&mut rt);
    let (x, y) = rt.find(&keys::palette_item(0)).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert_eq!(rt.state.theme, ThemeChoice::Light);

    shortcut(&mut rt, 'k');
    rt.ime(Ime::Commit("zzz".into()));
    settle(&mut rt);
    assert!(rt.find(keys::PALETTE_EMPTY).is_some(), "nothing matches: it says so");
    press(&mut rt, Key::Escape);
    assert!(rt.find(keys::PALETTE_INPUT).is_none(), "Escape closes it");
    assert!(!rt.state.palette.open);
}
