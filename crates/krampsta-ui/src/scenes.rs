//! Headless scenes for the newer states (`--scene NAME`); the table and the rest are in
//! `app.rs`. Each assigns the state it shows.

use krampsta_core::{AgentEvent, FileWrite, MessageRole, ToolCall, ToolCallId, ToolResult};

use crate::app::{SCENE_RUN, apply, scene_completed, staged};
use crate::menus::{Confirm, MenuTarget};
use crate::state::KrampstaApp;

/// The agent working through its plan: one step done, one under way, two to go.
pub fn plan(s: &mut KrampstaApp) {
    let run = staged(s, "Make the parser faster on the big fixture");
    let id = ToolCallId("scene-plan".into());
    let arguments = serde_json::json!({"steps": [
        {"step": "Profile the parser on fixtures/big.rs", "status": "completed"},
        {"step": "Cache the token table in Lexer::new", "status": "in_progress"},
        {"step": "Add a benchmark for the lexer", "status": "pending"},
        {"step": "Run the tests", "status": "pending"},
    ]})
    .to_string();
    let output = krampsta_agent::plan::run(&arguments).unwrap_or_default();
    let call = ToolCall { id: id.clone(), name: krampsta_agent::plan::TOOL.into(), arguments };
    let result = ToolResult { call_id: id, output, is_error: false };
    let text = "Most of the time goes to `Lexer::next`, which rebuilds the token table for every file.";
    apply(
        s,
        &[
            AgentEvent::ToolCallStarted { run, call },
            AgentEvent::ToolCallFinished { run, result },
            AgentEvent::TextDelta { run, text: text.into() },
        ],
    );
    s.catch_up();
}

/// A long conversation summed up before the last run, its summary open.
pub fn summary(s: &mut KrampstaApp) {
    s.new_session();
    let Some(x) = s.sessions.first_mut() else { return };
    x.push_message(MessageRole::User, "Profile the parser on the big fixture");
    x.push_message(
        MessageRole::Assistant,
        "Most of the time goes to `Lexer::next`, rebuilding the token table per file.",
    );
    let kept = x.push_message(MessageRole::User, "Cache the token table between files");
    x.push_message(
        MessageRole::Assistant,
        "Done: the table is built once in `Lexer::new` and shared; parsing the fixture went from 2.4 s to 0.9 s.",
    );
    let summary = "- The user wants the parser faster on `fixtures/big.rs`.\n- Profiling showed `Lexer::next` \
                   rebuilding the token table per file.\n- The table is now cached in `Lexer::new`: 2.4 s → 0.9 s.";
    if x.submit("Now add a benchmark for it", SCENE_RUN).is_ok() {
        for e in [
            AgentEvent::Started { run: SCENE_RUN },
            AgentEvent::Compacted { run: SCENE_RUN, summary: summary.into(), before: kept },
            AgentEvent::TextDelta { run: SCENE_RUN, text: "Added `benches/lexer.rs` with criterion.".into() },
            AgentEvent::Completed { run: SCENE_RUN },
        ] {
            x.apply(&e);
        }
    }
    let id = x.messages.iter().find(|m| m.role == MessageRole::Summary).map(|m| m.id);
    s.open_reasoning.extend(id);
}

/// Asking to rewind a finished turn that wrote two files.
pub fn rewind(s: &mut KrampstaApp) {
    scene_completed(s);
    let Some(x) = s.sessions.first_mut() else { return };
    let Some(turn) = x.messages.iter().find(|m| m.role == MessageRole::User).map(|m| m.id) else { return };
    for path in ["crates/krampsta-app/src/main.rs", "crates/krampsta-app/src/config.rs"] {
        x.record_write(FileWrite { turn, path: path.into(), before: Some("old".into()), after: "new".into() });
    }
    s.confirm = Some(Confirm::Rewind { session: x.id, message: turn });
}

/// The menu of the user's own message, with "Rewind to here…".
pub fn user_message_menu(s: &mut KrampstaApp) {
    scene_completed(s);
    let mine = s.current().and_then(|x| x.messages.iter().find(|m| m.role == MessageRole::User)).map(|m| m.id);
    if let Some(id) = mine {
        s.open_menu(MenuTarget::Message(id), rune::PointerPress { x: 560.0, y: 100.0 });
    }
}
