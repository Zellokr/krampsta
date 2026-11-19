//! The main pane: the transcript of the selected session and the composer under it.

use krampsta_core::{AgentStatus, MessageRole, Session};
use rune::{El, GrowthSpec, ScrollSpec, col, row, text};
use rune_kit::Kit;

use crate::components::alert::{Alert, Tone};
use crate::components::empty::Empty;
use crate::components::{composer, icons, menus, message, shimmer};
use crate::keys;
use crate::state::KrampstaApp;
use crate::theme;

const COLUMN: f32 = 820.0;

pub fn view(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>) -> El {
    let (content, running) = match s.current() {
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
    let composer = col().w_full().items_center().px(28.0).pt(8.0).pb(18.0).child(composer::view(s, running, k, layer));
    col().w_full().h_full().min_h(0.0).child(scroll).child(composer)
}

fn transcript(s: &KrampstaApp, session: &Session, k: &Kit, layer: &mut Vec<El>) -> El {
    let running = session.status.is_running();
    // Streaming while its run writes it, and after, until the pace has shown all of it.
    let streaming_id = session
        .messages
        .last()
        .filter(|m| {
            m.role == MessageRole::Assistant && ((running && m.run == session.active_run()) || s.is_pacing(m.id))
        })
        .map(|m| m.id);
    // A growth stack: a message that grows (streaming) eases to its new height instead of
    // jumping a line at a time, and the view pinned to the bottom follows it smoothly.
    let mut list = col()
        .key(keys::TRANSCRIPT)
        .role("log")
        .label("Conversation")
        .w_full()
        .max_w(COLUMN)
        .gap(24.0)
        .growth(GrowthSpec::default())
        .children(session.messages.iter().map(|m| {
            let streaming = Some(m.id) == streaming_id;
            let view = message::view(s, m, streaming, k);
            let row = menus::message(s, m, view, k, layer);
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
