#![allow(clippy::unwrap_used, clippy::expect_used)]

//! A run that ends while the window is behind calls the person back: the taskbar button
//! flashes and counts the session, and bringing the window back stops it. The window's focus
//! comes through Rune's own event (`Runtime::window_focus`), as the platform sends it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use krampsta_agent::{MockCatalog, MockConfig};
use krampsta_ui::attention::WindowAttention;
use krampsta_ui::{KrampstaApp, app};
use rune::{Env, Runtime};

/// Records what the window was asked to do.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

impl WindowAttention for Recorder {
    fn flash(&self, on: bool) {
        self.0.lock().unwrap().push(format!("flash {on}"));
    }
    fn badge(&self, count: u32, _: &str) {
        self.0.lock().unwrap().push(format!("badge {count}"));
    }
}

fn settle(rt: &mut Runtime<KrampstaApp>) {
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
}

fn run_to_the_end(rt: &mut Runtime<KrampstaApp>) {
    rt.update(|s, _| {
        s.new_session();
        s.set_draft("list the files");
        s.send().unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while rt.state.current().is_some_and(|s| s.status.is_running()) {
        assert!(Instant::now() < deadline, "the agent never finished");
        rt.update_if(|s, _| s.pump());
        rt.frame(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(2));
    }
    settle(rt);
}

fn runtime(recorder: &Recorder) -> Runtime<KrampstaApp> {
    let state =
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant()))).with_attention(Box::new(recorder.clone()));
    let mut rt = Runtime::new(state, app::view, (1200.0, 800.0), Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

#[test]
fn a_run_that_ends_behind_flashes_the_taskbar_until_the_window_is_back() {
    let recorder = Recorder::default();
    let mut rt = runtime(&recorder);
    rt.window_focus(false);
    settle(&mut rt);
    run_to_the_end(&mut rt);
    assert_eq!(*recorder.0.lock().unwrap(), ["flash true", "badge 1"], "behind: called back once");

    rt.window_focus(true);
    settle(&mut rt);
    assert_eq!(recorder.0.lock().unwrap()[2..], ["flash false", "badge 0"], "back in front: over");
}

#[test]
fn a_run_that_ends_in_front_calls_no_one() {
    let recorder = Recorder::default();
    let mut rt = runtime(&recorder);
    run_to_the_end(&mut rt);
    assert!(recorder.0.lock().unwrap().is_empty());
}
