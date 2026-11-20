//! Paced streaming: a reply comes out at the rate it arrives, smoothed, instead of in the bursts
//! a provider sends it in (several words per event, at uneven times, with gaps between).
//!
//! The text that arrived is kept whole. A [`Pacer`] estimates how fast it arrives (a moving
//! average over [`RATE_WINDOW`]) and lets it out at that rate from a small reserve
//! ([`RESERVE`]) that bridges the gaps between bursts: a growing reserve speeds it up, a
//! shrinking one slows it, so it neither stops between bursts nor falls behind. The position
//! moves continuously; the view shows up to the last whole word it has passed, so Rune's word
//! reveal fades in words, not fragments. Frames are asked for only while text is waiting.

use std::time::Duration;

use krampsta_core::{MessageId, MessageRole, SessionId};
use rune::Tx;

use crate::state::KrampstaApp;

/// How long the arrival rate is averaged over, in seconds.
const RATE_WINDOW: f64 = 0.8;
/// How much text is held back to bridge the gaps between bursts, in seconds of the rate.
const RESERVE: f64 = 0.25;
/// How quickly the reserve is brought back to size, in seconds.
const CORRECT: f64 = 0.6;
/// The slowest the text comes out while any is waiting, in bytes per second.
const MIN_RATE: f64 = 60.0;
/// The longest a backlog takes to come out once the reply has fully arrived, in seconds.
const DRAIN: f64 = 0.35;
/// A frame step longer than this (the window was hidden, a stall) counts as this long.
const MAX_STEP: f64 = 0.1;
/// A message already this long when pacing starts (the user switched to a session mid-reply)
/// shows as it is instead of replaying.
const CATCH_UP: usize = 600;

/// The pacing arithmetic, apart from any text: where the shown text has got to, and how fast
/// the text has been arriving.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pacer {
    /// Bytes let out so far (fractional: a slow rate still moves every frame).
    pos: f64,
    /// Bytes that had arrived at the last step.
    seen: usize,
    /// The arrival rate, in bytes per second.
    rate: f64,
    clock: f64,
}

impl Pacer {
    pub fn new(start: usize, clock: f64) -> Self {
        #[expect(clippy::cast_precision_loss, reason = "a text length, far below 2^52")]
        let pos = start as f64;
        Pacer { pos, seen: start, rate: 0.0, clock }
    }

    /// Moves to `clock`, with `arrived` bytes of text so far, still `arriving` or not. Returns
    /// how many bytes may show.
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "text lengths, far below 2^52, and a position never negative"
    )]
    pub fn step(&mut self, clock: f64, arrived: usize, arriving: bool) -> usize {
        let dt = (clock - self.clock).clamp(0.0, MAX_STEP);
        self.clock = clock;
        let new = arrived.saturating_sub(self.seen) as f64;
        self.seen = arrived;
        // The first text sets the rate as if it came over one reserve; then a moving average.
        if self.rate == 0.0 && new > 0.0 {
            self.rate = new / RESERVE;
        } else if dt > 0.0 {
            self.rate += (new - self.rate * dt) / RATE_WINDOW;
        }
        let backlog = arrived as f64 - self.pos;
        if backlog > 0.0 && dt > 0.0 {
            let speed = if arriving {
                // Keep about one reserve's worth waiting: what arrives comes out at its own pace.
                let reserve = self.rate * RESERVE;
                (self.rate + (backlog - reserve) / CORRECT).max(MIN_RATE)
            } else {
                // All of it is here: let the rest out quickly, but not in one jump.
                (backlog / DRAIN).max(self.rate).max(MIN_RATE)
            };
            self.pos = (self.pos + speed * dt).min(arrived as f64);
        }
        self.pos as usize
    }
}

/// The message being paced in one session, and how much of it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pace {
    message: MessageId,
    pacer: Pacer,
    shown: usize,
}

impl KrampstaApp {
    /// How much of `message` (in `session`) the view shows: all of it, unless it is being paced.
    pub fn shown_len(&self, session: SessionId, message: MessageId, len: usize) -> usize {
        match self.pace_of(session) {
            Some(p) if p.message == message => p.shown.min(len),
            _ => len,
        }
    }

    /// Whether `message` (in `session`) is still coming out (arriving, or paced out after it
    /// arrived).
    pub fn is_pacing(&self, session: SessionId, message: MessageId) -> bool {
        self.pace_of(session).is_some_and(|p| p.message == message)
    }

    /// Shows the current session's streaming reply as far as it has arrived (scenes stage a reply
    /// mid-stream and capture its first frame).
    pub(crate) fn catch_up(&mut self) {
        let Some(session) = self.current() else { return };
        let id = session.id;
        let writing =
            session.messages.last().filter(|m| m.role == MessageRole::Assistant && session.status.is_running());
        let pace =
            writing.map(|m| Pace { message: m.id, pacer: Pacer::new(m.content.len(), 0.0), shown: m.content.len() });
        self.view_mut(id).pace = pace;
    }

    /// Steps the paced reply of every session on screen by one frame at `clock`; a session no
    /// longer on screen shows its reply whole when it comes back. True when more text shows.
    pub(crate) fn step_pace(&mut self, clock: f64, tx: &mut Tx) -> bool {
        let shown = self.visible_sessions();
        for (id, view) in &mut self.views {
            if !shown.contains(id) {
                view.pace = None;
            }
        }
        let mut grew = false;
        for id in shown {
            grew |= self.step_pace_of(id, clock, tx);
        }
        grew
    }

    fn step_pace_of(&mut self, id: SessionId, clock: f64, tx: &mut Tx) -> bool {
        // Fields, not methods: the session is read while its view's pace changes.
        let Some(session) = self.sessions.iter().find(|s| s.id == id) else { return false };
        let view = self.views.entry(id).or_default();
        // The reply the session's run is writing, if any.
        let writing = session.messages.last().filter(|m| {
            m.role == MessageRole::Assistant && session.status.is_running() && m.run == session.active_run()
        });
        let message = match (view.pace, writing) {
            // Still pacing this reply, and no newer one has started.
            (Some(p), w) if w.is_none_or(|w| w.id == p.message) => {
                session.messages.iter().rev().find(|m| m.id == p.message)
            }
            // A reply to start pacing: from the top, unless it's already long (caught up mid-way).
            (_, Some(m)) => {
                let start = if m.content.len() > CATCH_UP { m.content.len() } else { 0 };
                let pacer = Pacer::new(start, clock);
                view.pace = Some(Pace { message: m.id, pacer, shown: start });
                Some(m)
            }
            _ => None,
        };
        let (Some(message), Some(pace)) = (message, view.pace.as_mut()) else {
            view.pace = None;
            return false;
        };
        let (text, arriving) = (&message.content, writing.is_some_and(|w| w.id == message.id));
        let at = pace.pacer.step(clock, text.len(), arriving);
        if at >= text.len() && !arriving {
            view.pace = None; // Arrived and shown: done.
            return true;
        }
        let shown = words_up_to(text, at, arriving).max(pace.shown);
        trace(clock, text.len(), at, shown, pace.pacer.rate, arriving);
        tx.wake_after(Duration::ZERO);
        let grew = shown != pace.shown;
        pace.shown = shown;
        grew
    }
}

/// With `KRAMPSTA_TRACE_PACE=1`, log pacing metrics to stderr, never the text.
fn trace(clock: f64, arrived: usize, at: usize, shown: usize, rate: f64, arriving: bool) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("KRAMPSTA_TRACE_PACE").is_some_and(|v| v == "1")) {
        let state = if arriving { "arriving" } else { "draining" };
        eprintln!("pace t={clock:.3} arrived={arrived} at={at} shown={shown} rate={rate:.0}B/s {state}");
    }
}

/// How much of `text` shows with the pace at `at`: up to the end of the last whole word before
/// it, on a character boundary. While the text still arrives, a word at its very end waits for
/// what follows it (it may not be whole yet).
fn words_up_to(text: &str, at: usize, arriving: bool) -> usize {
    if at >= text.len() {
        return if arriving { text.trim_end_matches(|c: char| !c.is_whitespace()).len() } else { text.len() };
    }
    let mut end = at;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    // Up to and including the space that ends the last whole word.
    text[..end].rfind(char::is_whitespace).map_or(0, |n| n + text[n..].chars().next().map_or(1, char::len_utf8))
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

    /// Steps one frame; the shown length after it.
    fn frame(app: &mut KrampstaApp, clock: &mut f64) -> usize {
        *clock += FRAME;
        app.step_pace(*clock, &mut Tx::default());
        let (id, len) = reply(app);
        app.shown_len(app.selected, id, len)
    }

    fn frames(app: &mut KrampstaApp, clock: &mut f64, n: usize) -> Vec<usize> {
        (0..n).map(|_| frame(app, clock)).collect()
    }

    #[test]
    fn a_bursty_stream_comes_out_steadily_without_stalling_between_bursts() {
        // As a backend sends it: 30 bytes (six words) every 150 ms, for three seconds.
        let mut app = app();
        let mut clock = 0.0;
        let mut shown = Vec::new();
        for n in 0..180 {
            if n % 9 == 0 {
                arrive(&mut app, AgentEvent::TextDelta { run: RUN, text: "word ".repeat(6) });
            }
            shown.push(frame(&mut app, &mut clock));
        }
        let steady = &shown[30..]; // After the first half second.
        let longest_stall = steady
            .windows(2)
            .fold((0usize, 0usize), |(run, worst), w| {
                let run = if w[1] > w[0] { 0 } else { run + 1 };
                (run, worst.max(run))
            })
            .1;
        assert!(longest_stall <= 3, "a word at least every 4 frames, never a pause per burst: {longest_stall}");
        let (_, arrived) = reply(&app);
        let behind = arrived - shown[179];
        assert!(behind <= 120, "it keeps up: {behind} bytes behind (about a burst and a reserve)");
    }

    #[test]
    fn a_burst_comes_out_over_several_frames_a_word_at_a_time() {
        let mut app = app();
        let burst = "word ".repeat(60);
        arrive(&mut app, AgentEvent::TextDelta { run: RUN, text: burst.clone() });
        let mut clock = 0.0;
        let shown = frames(&mut app, &mut clock, 120);
        assert!(shown[1] < burst.len(), "not all at once: {}", shown[1]);
        assert!(shown.windows(2).all(|w| w[0] <= w[1]), "it only grows");
        assert!(shown.iter().all(|&n| n == 0 || burst[..n].ends_with(' ')), "whole words");
        let done = shown.iter().position(|&n| n == burst.len()).expect("all of it shows");
        assert!(done <= 90, "out in well under two seconds: frame {done}");
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
        frames(&mut app, &mut clock, 60);
        assert_eq!(app.pace_of(app.selected), None, "done once all of it showed, within a second of the end");
        let (id, len) = reply(&app);
        assert_eq!(app.shown_len(app.selected, id, len), len);
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
        assert_eq!(app.pace_of(app.selected), None);
    }

    #[test]
    fn only_whole_words_show_and_a_trailing_word_waits_while_arriving() {
        assert_eq!(words_up_to("hello world", 3, true), 0);
        assert_eq!(words_up_to("hello world", 8, true), 6);
        assert_eq!(words_up_to("hello world", 11, true), 6, "`world` may not be whole yet");
        assert_eq!(words_up_to("hello world", 11, false), 11);
        assert_eq!(words_up_to("ñandú y más", 8, true), "ñandú ".len(), "on a character boundary");
    }
}
