//! Paced streaming: a reply is shown at a steady rate from what has arrived, instead of in the
//! bursts a provider sends it in (several words per event, at uneven times).
//!
//! The text that arrived is kept whole; the view shows a prefix that grows every frame, by the
//! backlog over a short lag (so a burst spreads over a few frames and a trickle reads word by
//! word), and never by less than a floor. It grows a whole word at a time, so Rune's word
//! reveal fades in words, not fragments. Frames are asked for only while text is waiting.

use std::time::Duration;

use krampsta_core::{MessageId, MessageRole, SessionId};
use rune::Tx;

use crate::state::KrampstaApp;

/// How far behind the arriving text the shown text runs, at most, under a steady stream: the
/// backlog drains with this time constant.
const LAG: f64 = 0.15;
/// The slowest the text comes out while any is waiting, in bytes per second.
const MIN_RATE: f64 = 180.0;
/// A frame step longer than this (the window was hidden, a stall) counts as this long.
const MAX_STEP: f64 = 0.1;
/// A message already this long when pacing starts (the user switched to a session mid-reply)
/// shows as it is instead of replaying.
const CATCH_UP: usize = 600;

/// The message being paced, and how much of it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pace {
    session: SessionId,
    message: MessageId,
    shown: usize,
    clock: f64,
}

impl KrampstaApp {
    /// How much of `message` the view shows: all of it, unless it is being paced.
    pub fn shown_len(&self, message: MessageId, len: usize) -> usize {
        match self.pace {
            Some(p) if p.message == message && p.session == self.selected => p.shown.min(len),
            _ => len,
        }
    }

    /// Whether `message` is still coming out (arriving, or paced out after it arrived).
    pub fn is_pacing(&self, message: MessageId) -> bool {
        self.pace.is_some_and(|p| p.message == message && p.session == self.selected)
    }

    /// Shows the current session's streaming reply as far as it has arrived (scenes stage a reply
    /// mid-stream and capture its first frame).
    pub(crate) fn catch_up(&mut self) {
        let Some(session) = self.current() else { return };
        let writing =
            session.messages.last().filter(|m| m.role == MessageRole::Assistant && session.status.is_running());
        self.pace = writing.map(|m| Pace { session: session.id, message: m.id, shown: m.content.len(), clock: 0.0 });
    }

    /// Steps the paced message by one frame at `clock`. True when more of it shows.
    pub(crate) fn step_pace(&mut self, clock: f64, tx: &mut Tx) -> bool {
        // Fields, not methods: the session is read while `pace` changes.
        let selected = self.selected;
        let Some(session) = self.sessions.iter().find(|s| s.id == selected) else {
            self.pace = None;
            return false;
        };
        // The reply the current session's run is writing, if any.
        let writing = session.messages.last().filter(|m| {
            m.role == MessageRole::Assistant && session.status.is_running() && m.run == session.active_run()
        });
        let message = match (self.pace, writing) {
            // Still pacing this session's reply, and no newer one has started.
            (Some(p), w) if p.session == session.id && w.is_none_or(|w| w.id == p.message) => {
                session.messages.iter().rev().find(|m| m.id == p.message)
            }
            // A reply to start pacing: from the top, unless it's already long (caught up mid-way).
            (_, Some(m)) => {
                let shown = if m.content.len() > CATCH_UP { m.content.len() } else { 0 };
                self.pace = Some(Pace { session: session.id, message: m.id, shown, clock });
                Some(m)
            }
            _ => None,
        };
        let (Some(message), Some(pace)) = (message, self.pace.as_mut()) else {
            self.pace = None;
            return false;
        };
        let (text, still_writing) = (&message.content, writing.is_some_and(|w| w.id == message.id));
        let step = (clock - pace.clock).clamp(0.0, MAX_STEP);
        pace.clock = clock;
        let backlog = text.len().saturating_sub(pace.shown);
        if backlog == 0 {
            if !still_writing {
                self.pace = None; // Arrived and shown: done.
            }
            return false;
        }
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a byte count per frame, far below 2^52"
        )]
        let advance = ((backlog as f64 / LAG).max(MIN_RATE) * step).ceil() as usize;
        pace.shown = word_end(text, pace.shown + advance.max(1));
        tx.wake_after(Duration::ZERO);
        true
    }
}

/// The end of the word `at` falls in (or `at` itself at a word's end), on a character boundary,
/// never past the text.
fn word_end(text: &str, at: usize) -> usize {
    if at >= text.len() {
        return text.len();
    }
    let mut end = at;
    while !text.is_char_boundary(end) {
        end += 1;
    }
    text[end..].find(char::is_whitespace).map_or(text.len(), |n| end + n)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::MockCatalog;
    use krampsta_core::{AgentEvent, RunId};

    use super::*;

    const FRAME: f64 = 1.0 / 60.0;
    const RUN: RunId = RunId(9);

    /// A new session whose run has started.
    fn app() -> KrampstaApp {
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::default()));
        app.new_session();
        let session = app.sessions.first_mut().unwrap();
        session.submit("hi", RUN).unwrap();
        session.apply(&AgentEvent::Started { run: RUN });
        app
    }

    fn arrive(app: &mut KrampstaApp, event: AgentEvent) {
        app.sessions.first_mut().unwrap().apply(&event);
    }

    fn reply(app: &KrampstaApp) -> (MessageId, usize) {
        let m = app.sessions[0].messages.last().unwrap();
        (m.id, m.content.len())
    }

    /// Steps `frames` frames from `clock`; the shown length after each.
    fn frames(app: &mut KrampstaApp, clock: &mut f64, frames: usize) -> Vec<usize> {
        (0..frames)
            .map(|_| {
                *clock += FRAME;
                app.step_pace(*clock, &mut Tx::default());
                let (id, len) = reply(app);
                app.shown_len(id, len)
            })
            .collect()
    }

    #[test]
    fn a_burst_comes_out_over_several_frames_a_word_at_a_time() {
        let mut app = app();
        let burst = "word ".repeat(60);
        arrive(&mut app, AgentEvent::TextDelta { run: RUN, text: burst.clone() });
        let mut clock = 0.0;
        let shown = frames(&mut app, &mut clock, 120);
        assert!(shown[1] > 0 && shown[1] < burst.len(), "neither nothing nor all at once: {}", shown[1]);
        assert!(shown.windows(2).all(|w| w[0] <= w[1]), "it only grows");
        assert!(shown.iter().all(|&n| n == burst.len() || burst[n..].starts_with(' ')), "whole words");
        let done = shown.iter().position(|&n| n == burst.len()).expect("all of it shows");
        assert!((5..=60).contains(&done), "spread over a few frames, well under a second: {done}");
    }

    #[test]
    fn text_arriving_later_continues_from_where_it_was_and_the_end_is_paced_out() {
        let mut app = app();
        let mut clock = 0.0;
        arrive(&mut app, AgentEvent::TextDelta { run: RUN, text: "first part ".repeat(10) });
        frames(&mut app, &mut clock, 120);
        let (_, before) = reply(&app);
        arrive(&mut app, AgentEvent::TextDelta { run: RUN, text: "then more ".repeat(10) });
        arrive(&mut app, AgentEvent::Completed { run: RUN });
        let shown = frames(&mut app, &mut clock, 1);
        assert!(shown[0] >= before && shown[0] < reply(&app).1, "no replay, no jump to the end");
        frames(&mut app, &mut clock, 120);
        assert_eq!(app.pace, None, "done once all of it showed");
        let (id, len) = reply(&app);
        assert_eq!(app.shown_len(id, len), len);
    }

    #[test]
    fn a_reply_already_long_when_pacing_starts_shows_as_it_is() {
        let mut app = app();
        arrive(&mut app, AgentEvent::TextDelta { run: RUN, text: "x ".repeat(CATCH_UP) });
        let mut clock = 0.0;
        let (_, len) = reply(&app);
        assert_eq!(frames(&mut app, &mut clock, 1), [len], "caught up mid-reply: no replay");
    }

    #[test]
    fn nothing_is_paced_without_a_reply_streaming() {
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::default()));
        assert!(!app.step_pace(1.0, &mut Tx::default()));
        assert_eq!(app.pace, None);
    }

    #[test]
    fn steps_end_on_whole_words_and_characters() {
        assert_eq!(word_end("hello world", 2), 5);
        assert_eq!(word_end("hello world", 5), 5);
        assert_eq!(word_end("hello world", 7), 11);
        assert_eq!(word_end("ñandú y más", 1), "ñandú".len(), "inside a multi-byte character");
        assert_eq!(word_end("short", 99), 5);
    }
}
