#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The Copy button on a reply's code block: a click puts the block's code on the clipboard and
//! the button says so.

use std::sync::Arc;

use krampsta_agent::{MockCatalog, MockConfig};
use krampsta_core::{MessageRole, Session, SessionId};
use krampsta_ui::{KrampstaApp, app, keys};
use rune::{Env, Runtime};

fn settle(rt: &mut Runtime<KrampstaApp>) {
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
}

#[test]
fn a_click_copies_the_blocks_code_and_the_button_says_so() {
    let mut state = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())));
    let mut session = Session::with_title(SessionId(7), "Show me main");
    session.push_message(MessageRole::User, "Show me main");
    let reply = session.push_message(
        MessageRole::Assistant,
        "Here it is:\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nAnd the test:\n\n```sh\ncargo test\n```\n",
    );
    state.sessions.insert(0, session);
    state.select(SessionId(7));
    let mut rt = Runtime::new(state, app::view, (1200.0, 800.0), Env::new(true, 1.0));
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    settle(&mut rt);
    assert!(rt.find(&keys::code_copy(reply, 0)).is_some() && rt.find(&keys::code_copy(reply, 1)).is_some());

    let (x, y) = rt.find(&keys::code_copy(reply, 1)).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    let copied = clipboard.text().map(|t| t.to_string()).unwrap_or_default();
    assert_eq!(copied.trim_end(), "cargo test", "the second block's code, as written");
    assert_eq!(rt.state.copied_code, Some((reply, 1)), "its button says Copied");

    let (x, y) = rt.find(&keys::code_copy(reply, 0)).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    let copied = clipboard.text().map(|t| t.to_string()).unwrap_or_default();
    assert!(copied.starts_with("fn main() {") && copied.contains("println!(\"hi\");"), "{copied}");
    assert_eq!(rt.state.copied_code, Some((reply, 0)), "only the last one says so");
}
