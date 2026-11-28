//! A conversation pane: one session's transcript and the composer under it.

use krampsta_core::{AgentStatus, MessageRole, Session, SessionId};
use rune::{El, GrowthSpec, ScrollSpec, col, row, text};
use rune_kit::Kit;

use crate::components::alert::{Alert, Tone};
use crate::components::empty::Empty;
use crate::components::{composer, icons, menus, message, shimmer};
use crate::keys;
use crate::state::KrampstaApp;
use crate::theme;

const COLUMN: f32 = 820.0;

pub fn view(s: &KrampstaApp, id: SessionId, k: &Kit, layer: &mut Vec<El>) -> El {
    let (content, running) = match s.session(id) {
        Some(session) if !session.messages.is_empty() => {
            (transcript(s, session, k, layer), session.status.is_running())
        }
        Some(session) => (empty_state(k), session.status.is_running()),
        None => (empty_state(k), false),
    };
    let scroll = col()
        .key(keys::TRANSCRIPT_SCROLL)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_edge_fade(12.0)
        .child(col().w_full().items_center().px(28.0).pt(24.0).pb(16.0).child(content));
    let composer =
        col().w_full().items_center().px(28.0).pt(8.0).pb(18.0).child(composer::view(s, id, running, k, layer));
    col().w_full().h_full().min_h(0.0).child(scroll).child(composer)
}

fn transcript(s: &KrampstaApp, session: &Session, k: &Kit, layer: &mut Vec<El>) -> El {
    let running = session.status.is_running();
    // Streaming while its run writes it, and after, until the pace has shown all of it.
    let streaming_id = session
        .messages
        .last()
        .filter(|m| {
            m.role == MessageRole::Assistant
                && ((running && m.run == session.active_run()) || s.is_pacing(session.id, m.id))
        })
        .map(|m| m.id);
    // A growth stack: a message that grows (streaming) eases to its new height instead of
    // jumping, and the view pinned to the bottom follows it. The word-fade edge draws the text as
    // it comes out, unclipped, each word fading in: the feather edges clip the row at its eased
    // height, which hid each new line until the height caught up (text came out a line at a time).
    let mut list = col()
        .key(keys::TRANSCRIPT)
        .role("log")
        .label("Conversation")
        .w_full()
        .max_w(COLUMN)
        .gap(24.0)
        .growth(GrowthSpec { spring: rune::GROWTH, edge: rune::edge::WORD_FADE })
        .children(session.messages.iter().map(|m| {
            let streaming = Some(m.id) == streaming_id;
            let view = message::view(s, session.id, m, streaming, k);
            let row = menus::message(s, session, m, view, k, layer);
            if streaming { row.streaming(true) } else { row.arrives_whole() }
        }));
    // Running, but the assistant hasn't started its message yet: show that it's working.
    if running && streaming_id.is_none() {
        list = list.child(waiting(k));
    }
    match &session.status {
        AgentStatus::Failed { error } => list = list.child(run_error(error, k)),
        AgentStatus::Cancelled => {
            list = list.child(text("Run cancelled.").font(k.ty.small).color(k.c.text_faint).key(keys::RUN_ERROR))
        }
        _ => {}
    }
    list
}

fn waiting(k: &Kit) -> El {
    row()
        .gap(8.0)
        .items_center()
        .child(rune_kit::Spinner::new().size(12.0).color(theme::accent(k.dark)).build(k))
        .child(shimmer::view(keys::STARTING, "Starting…", k.ty.ui, k.c.text_faint, k.c.text_base))
}

fn run_error(error: &str, k: &Kit) -> El {
    Alert::new(Tone::Danger, "The run failed").description(error).build(k).key(keys::RUN_ERROR)
}

fn empty_state(k: &Kit) -> El {
    let empty = Empty::new(&icons::KRAMPSTA, "What are we building today?")
        .large()
        .mark_color(theme::accent(k.dark))
        .description("Ask Krampsta to explore the repository, explain code or plan a change.")
        .build(k);
    col().key(keys::EMPTY_STATE).w_full().max_w(COLUMN).pt(120.0).child(empty)
}
