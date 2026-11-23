//! Telling the person, while the window is in the background, that a session needs them: a
//! run ended, or a command waits for permission. The window's taskbar button flashes (and stays
//! lit until the window comes to the front) and counts those sessions in a badge; bringing the
//! window back clears both. Rune draws them (`WindowHandle::flash` and `set_taskbar`) on Windows,
//! the badge on macOS' Dock; elsewhere they do nothing. The window says when it gains or loses
//! focus through the root's `on_window_focus`.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use krampsta_core::SessionId;

use crate::state::KrampstaApp;

/// What the window can do to call the person back. A fake in tests.
pub trait WindowAttention {
    /// Lights the taskbar button (until the window comes to the front), or stops.
    fn flash(&self, on: bool);
    /// A count on the button; 0 shows none. `description` is what a screen reader says.
    fn badge(&self, count: u32, description: &str);
}

/// The app's window, once Rune has made it (`App::on_window` fills it in).
pub type WindowSlot = Rc<RefCell<Option<rune::WindowHandle>>>;

impl WindowAttention for WindowSlot {
    fn flash(&self, on: bool) {
        if let Some(window) = self.borrow().as_ref() {
            window.flash(on);
        }
    }

    fn badge(&self, count: u32, description: &str) {
        if let Some(window) = self.borrow().as_ref() {
            window.set_taskbar(rune::Taskbar::new().badge(count, description));
        }
    }
}

/// Whether the window is in front, and the sessions that need the person since it wasn't.
pub struct Attention {
    window: Option<Box<dyn WindowAttention>>,
    pub focused: bool,
    pub waiting: HashSet<SessionId>,
}

impl Default for Attention {
    fn default() -> Self {
        // A window starts in front; one that never says otherwise never flashes.
        Attention { window: None, focused: true, waiting: HashSet::new() }
    }
}

impl KrampstaApp {
    /// Lets the app call the person back through `window` (the real one, or a fake).
    pub fn with_attention(mut self, window: Box<dyn WindowAttention>) -> Self {
        self.attention.window = Some(window);
        self
    }

    /// The window came to the front or went behind. In front, the call back is over.
    pub fn window_focus(&mut self, focused: bool) {
        self.attention.focused = focused;
        if focused && !self.attention.waiting.is_empty() {
            self.attention.waiting.clear();
            if let Some(window) = &self.attention.window {
                window.flash(false);
                window.badge(0, "");
            }
        }
    }

    /// `session` needs the person (its run ended, or it asks for permission). Behind, the
    /// window calls them back; in front, they can see it already.
    pub(crate) fn needs_attention(&mut self, session: SessionId) {
        if self.attention.focused || !self.attention.waiting.insert(session) {
            return;
        }
        let count = self.attention.waiting.len();
        if let Some(window) = &self.attention.window {
            window.flash(true);
            let description = match count {
                1 => "A session needs you".to_owned(),
                n => format!("{n} sessions need you"),
            };
            window.badge(u32::try_from(count).unwrap_or(u32::MAX), &description);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use krampsta_agent::{MockCatalog, MockConfig};

    use super::*;

    /// Records what the window was asked to do.
    #[derive(Clone, Default)]
    pub(crate) struct Recorder(pub Arc<Mutex<Vec<String>>>);

    impl WindowAttention for Recorder {
        fn flash(&self, on: bool) {
            self.0.lock().unwrap().push(format!("flash {on}"));
        }
        fn badge(&self, count: u32, description: &str) {
            self.0.lock().unwrap().push(format!("badge {count} {description}"));
        }
    }

    fn app(recorder: &Recorder) -> KrampstaApp {
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant()))).with_attention(Box::new(recorder.clone()))
    }

    #[test]
    fn in_front_nothing_flashes() {
        let recorder = Recorder::default();
        let mut app = app(&recorder);
        app.needs_attention(SessionId(1));
        assert!(recorder.0.lock().unwrap().is_empty());
    }

    #[test]
    fn behind_it_flashes_and_counts_each_session_once_until_it_comes_back() {
        let recorder = Recorder::default();
        let mut app = app(&recorder);
        app.window_focus(false);
        app.needs_attention(SessionId(1));
        app.needs_attention(SessionId(1));
        app.needs_attention(SessionId(2));
        app.window_focus(true);
        assert_eq!(
            *recorder.0.lock().unwrap(),
            [
                "flash true",
                "badge 1 A session needs you",
                "flash true",
                "badge 2 2 sessions need you",
                "flash false",
                "badge 0 ",
            ]
        );
        assert!(app.attention.waiting.is_empty());
    }
}
