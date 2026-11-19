//! Interaction tests: the real Krampsta view in Rune's headless `Runtime`, driven by pointer and
//! keyboard input the way a person would, with the mock agent running for real.

use std::sync::Arc;
use std::time::{Duration, Instant};

use krampsta_agent::{MockCatalog, MockConfig};
use krampsta_core::AgentStatus;
use krampsta_ui::picker::Picker;
use krampsta_ui::{KrampstaApp, Screen, app, keys};
use rune::runtime::ElementInfo;
use rune::{Env, Ime, Key, KeyEvent, Mods, Runtime};

const SIZE: (f32, f32) = (1200.0, 800.0);

fn runtime_with(config: MockConfig, size: (f32, f32)) -> Runtime<KrampstaApp> {
    let state = KrampstaApp::new(Arc::new(MockCatalog::new(config)));
    let mut rt = Runtime::new(state, app::view, size, Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

fn runtime() -> Runtime<KrampstaApp> {
    runtime_with(MockConfig::instant(), SIZE)
}

fn settle(rt: &mut Runtime<KrampstaApp>) {
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
}

fn element(rt: &Runtime<KrampstaApp>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting)
}

fn has(rt: &Runtime<KrampstaApp>, key: &str) -> bool {
    element(rt, key).is_some()
}

fn click(rt: &mut Runtime<KrampstaApp>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn type_text(rt: &mut Runtime<KrampstaApp>, text: &str) {
    rt.ime(Ime::Commit(text.into()));
    rt.frame(1.0 / 60.0);
}

fn press(rt: &mut Runtime<KrampstaApp>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.frame(1.0 / 60.0);
}

/// Runs `poll` (what the agent host's waker triggers in the app) until the run ends.
fn pump_until_finished(rt: &mut Runtime<KrampstaApp>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while rt.state.current().is_some_and(|s| s.status.is_running()) {
        assert!(Instant::now() < deadline, "the agent never finished");
        rt.update_if(|s, _| s.pump());
        rt.frame(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(2));
    }
    settle(rt);
}

fn status_label(rt: &Runtime<KrampstaApp>) -> String {
    element(rt, keys::STATUS).and_then(|e| e.label).unwrap_or_default()
}

fn inside(inner: [f32; 4], outer: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 0.5
        && inner[1] >= outer[1] - 0.5
        && inner[0] + inner[2] <= outer[0] + outer[2] + 0.5
        && inner[1] + inner[3] <= outer[1] + outer[3] + 0.5
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

#[test]
fn the_window_has_sidebar_chat_composer_and_changes() {
    let rt = runtime();
    for key in [
        keys::BRAND,
        keys::NEW_SESSION,
        keys::SETTINGS,
        keys::TRANSCRIPT,
        keys::COMPOSER,
        keys::SEND,
        keys::STATUS,
        keys::CHANGES,
        keys::CHANGES_TOGGLE,
    ] {
        assert!(has(&rt, key), "missing {key}");
    }
    for s in &rt.state.sessions {
        assert!(has(&rt, &keys::session(s.id)), "sidebar row for {}", s.title);
    }
    assert_eq!(status_label(&rt), "Agent idle");
}

#[test]
fn new_session_adds_a_selected_row_with_an_empty_conversation() {
    let mut rt = runtime();
    let before = rt.state.sessions.len();
    click(&mut rt, keys::NEW_SESSION);
    assert_eq!(rt.state.sessions.len(), before + 1);
    let id = rt.state.selected;
    assert_eq!(rt.state.sessions[0].id, id);
    let row = element(&rt, &keys::session(id)).expect("the new row");
    assert!(row.item_selected, "the new session is the selected row");
    assert!(has(&rt, keys::EMPTY_STATE));
}

#[test]
fn clicking_a_session_selects_it() {
    let mut rt = runtime();
    let other = rt.state.sessions[1].id;
    click(&mut rt, &keys::session(other));
    assert_eq!(rt.state.selected, other);
    let title = rt.state.session(other).map(|s| s.title.clone()).unwrap_or_default();
    assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some(title.as_str())), "the toolbar shows {title}");
}

#[test]
fn typing_and_pressing_enter_runs_the_agent_and_shows_the_reply() {
    let mut rt = runtime();
    click(&mut rt, keys::NEW_SESSION);
    click(&mut rt, keys::COMPOSER);
    type_text(&mut rt, "Summarise the workspace");
    assert_eq!(rt.state.draft.text, "Summarise the workspace", "the composer accepts text");

    press(&mut rt, Key::Enter, Mods::default());
    settle(&mut rt);
    let session = rt.state.current().cloned().expect("a session");
    assert_eq!(session.status, AgentStatus::Running);
    assert!(rt.state.draft.text.is_empty(), "the composer clears after sending");
    let user = &session.messages[0];
    assert!(has(&rt, &keys::message(user.id)), "the user's message is in the transcript");
    assert_eq!(status_label(&rt), "Agent running");
    assert!(has(&rt, keys::STOP), "send turns into stop while running");

    pump_until_finished(&mut rt);
    let session = rt.state.current().cloned().expect("a session");
    assert_eq!(session.status, AgentStatus::Completed);
    assert_eq!(status_label(&rt), "Agent completed");
    let reply = session.messages.last().expect("a reply");
    assert!(has(&rt, &keys::message(reply.id)), "the reply is in the transcript");
    assert!(has(&rt, keys::SEND), "stop turns back into send");
}

#[test]
fn a_tool_card_opens_and_closes_on_click() {
    let mut rt = runtime();
    click(&mut rt, keys::COMPOSER);
    type_text(&mut rt, "List the files");
    press(&mut rt, Key::Enter, Mods::default());
    pump_until_finished(&mut rt);
    let session = rt.state.current().cloned().expect("a session");
    let call = session.messages.iter().flat_map(|m| &m.tool_calls).next().expect("the mock calls a tool").clone();
    let header = keys::tool(&call.id);
    let shows_output = |rt: &Runtime<KrampstaApp>| rt.elements().iter().any(|e| e.text.as_deref() == Some("Output"));
    assert!(has(&rt, &header));
    assert!(!shows_output(&rt), "cards start closed");

    click(&mut rt, &header);
    assert!(rt.state.open_tools.contains(&call.id));
    assert!(shows_output(&rt), "an open card shows the tool's output");
    click(&mut rt, &header);
    assert!(!rt.state.open_tools.contains(&call.id));
    assert!(!shows_output(&rt));
}

fn right_click(rt: &mut Runtime<KrampstaApp>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    settle(rt);
}

#[test]
fn a_session_is_deleted_from_its_context_menu_after_confirming() {
    let mut rt = runtime();
    let (first, second) = (rt.state.sessions[0].id, rt.state.sessions[1].id);
    let menu = keys::session_menu(first);
    right_click(&mut rt, &keys::session(first));
    assert!(has(&rt, &format!("{menu}-menu")), "right-click opens the row's menu");

    // Items: Copy title, a separator, Delete….
    click(&mut rt, &format!("{menu}-item2"));
    assert!(!has(&rt, &format!("{menu}-menu")), "choosing closes the menu");
    let dialog = format!("{}-dialog", keys::DELETE_SESSION);
    assert!(has(&rt, &dialog), "deleting asks first");

    click(&mut rt, keys::DELETE_SESSION_CANCEL);
    assert!(rt.state.session(first).is_some(), "cancel keeps it");

    right_click(&mut rt, &keys::session(first));
    click(&mut rt, &format!("{menu}-item2"));
    click(&mut rt, keys::DELETE_SESSION_CONFIRM);
    assert!(rt.state.session(first).is_none());
    assert!(!has(&rt, &keys::session(first)));
    assert_eq!(rt.state.selected, second, "the next session is selected");
}

#[test]
fn the_send_button_sends_too() {
    let mut rt = runtime();
    click(&mut rt, keys::COMPOSER);
    type_text(&mut rt, "via the button");
    click(&mut rt, keys::SEND);
    pump_until_finished(&mut rt);
    assert_eq!(rt.state.current().map(|s| s.status.clone()), Some(AgentStatus::Completed));
}

#[test]
fn a_failed_run_shows_its_error() {
    let mut rt = runtime();
    click(&mut rt, keys::COMPOSER);
    type_text(&mut rt, "/fail on purpose");
    press(&mut rt, Key::Enter, Mods::default());
    pump_until_finished(&mut rt);
    assert_eq!(status_label(&rt), "Agent failed");
    assert!(has(&rt, keys::RUN_ERROR));
}

#[test]
fn stop_cancels_a_running_agent() {
    let slow = MockConfig { think: Duration::from_secs(30), ..MockConfig::default() };
    let mut rt = runtime_with(slow, SIZE);
    click(&mut rt, keys::COMPOSER);
    type_text(&mut rt, "slowly");
    press(&mut rt, Key::Enter, Mods::default());
    settle(&mut rt);
    click(&mut rt, keys::STOP);
    pump_until_finished(&mut rt);
    assert_eq!(status_label(&rt), "Agent cancelled");
}

#[test]
fn the_changes_panel_hides_and_shows() {
    let mut rt = runtime();
    assert!(rt.state.changes.is_open());
    click(&mut rt, keys::CHANGES_TOGGLE);
    assert!(!rt.state.changes.is_open());
    assert!(
        !has(&rt, keys::CHANGES) || element(&rt, keys::CHANGES).is_some_and(|e| e.rect[2] < 1.0 || e.opacity < 0.01)
    );
    click(&mut rt, keys::CHANGES_TOGGLE);
    assert!(rt.state.changes.is_open());
    assert!(element(&rt, keys::CHANGES).is_some_and(|e| e.rect[2] > 200.0));
}

#[test]
fn settings_open_from_the_sidebar_and_close_with_back() {
    let mut rt = runtime();
    click(&mut rt, keys::SETTINGS);
    assert_eq!(rt.state.screen, Screen::Settings);
    assert!(has(&rt, keys::SETTINGS_PAGE));
    click(&mut rt, keys::SETTINGS_BACK);
    assert_eq!(rt.state.screen, Screen::Chat);
    assert!(has(&rt, keys::COMPOSER));
}

#[test]
fn keyboard_shortcut_creates_a_session() {
    let mut rt = runtime();
    let before = rt.state.sessions.len();
    // `AppSpec::on_key` belongs to the app shell, which a bare `Runtime` doesn't include, so
    // the handler is called here as the shell calls it for a key no control took.
    let ev = KeyEvent { key: Key::Command('n'), mods: Mods { cmd: true, ..Mods::default() } };
    let mut handled = false;
    rt.update(|s, tx| handled = app::on_key(s, &ev, tx));
    settle(&mut rt);
    assert!(handled);
    assert_eq!(rt.state.sessions.len(), before + 1);
    assert!(has(&rt, keys::EMPTY_STATE));
}

/// Every important element stays inside the window and the toolbar's title never runs under
/// the status pill, from the minimum size up to a large display.
#[test]
fn the_layout_holds_at_different_sizes() {
    for size in [app::MIN_SIZE, (1000.0, 700.0), SIZE, (1680.0, 1050.0)] {
        let mut rt = runtime_with(MockConfig::instant(), size);
        rt.update(|s, _| {
            if let Some(x) = s.sessions.first_mut() {
                x.title = "A deliberately long session title that has to truncate in the toolbar".into();
            }
        });
        settle(&mut rt);
        let window = [0.0, 0.0, size.0, size.1];
        for key in [keys::COMPOSER, keys::STATUS, keys::TRANSCRIPT] {
            let e = element(&rt, key).unwrap_or_else(|| panic!("{key} at {size:?}"));
            assert!(inside(e.rect, window), "{key} {:?} leaves the {size:?} window", e.rect);
        }
        let composer = element(&rt, keys::COMPOSER).map(|e| e.rect).unwrap_or_default();
        assert!(composer[2] >= 280.0, "composer usable at {size:?}: {composer:?}");
        let pill = element(&rt, keys::STATUS).map(|e| e.rect).unwrap_or_default();
        let title = rt
            .elements()
            .into_iter()
            .find(|e| e.text.as_deref().is_some_and(|t| t.starts_with("A deliberately long")) && e.rect[1] < 60.0)
            .map(|e| e.visible)
            .unwrap_or_default();
        assert!(!overlaps(title, pill), "title {title:?} overlaps the pill {pill:?} at {size:?}");
    }
}

#[test]
fn the_model_picker_opens_from_the_composer_and_choosing_closes_it() {
    let mut rt = runtime();
    let trigger = format!("{}-trigger", keys::COMPOSER_MODEL);
    let listbox = format!("{}-content", keys::COMPOSER_MODEL);
    assert!(has(&rt, &trigger), "the model picker sits under the composer");
    assert!(!has(&rt, &listbox));

    click(&mut rt, &trigger);
    assert_eq!(rt.state.engine.open, Some(Picker::ComposerModel));
    let list = element(&rt, &listbox).expect("the listbox opens");
    assert!(inside(list.rect, [0.0, 0.0, SIZE.0, SIZE.1]), "the listbox stays in the window: {:?}", list.rect);
    let option = element(&rt, "option0").expect("the model is listed");
    assert!(option.visible[2] > 0.0 && option.visible[3] > 0.0, "the option is on screen");

    click(&mut rt, "option0");
    assert_eq!(rt.state.engine.open, None, "choosing closes the list");
    assert_eq!(rt.state.engine.selection.model.as_deref(), Some("simulated"));
}

#[test]
fn escape_closes_an_open_picker_before_leaving_settings() {
    let mut rt = runtime();
    rt.update(|s, _| s.screen = Screen::Settings);
    settle(&mut rt);
    click(&mut rt, &format!("{}-trigger", keys::SETTINGS_PROVIDER));
    assert_eq!(rt.state.engine.open, Some(Picker::SettingsProvider));
    rt.update_if(|s, tx| app::on_key(s, &KeyEvent { key: Key::Escape, mods: Mods::default() }, tx));
    settle(&mut rt);
    assert_eq!(rt.state.engine.open, None);
    assert_eq!(rt.state.screen, Screen::Settings, "the first Escape only closes the list");
}
