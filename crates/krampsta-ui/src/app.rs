//! Rune wiring: the app spec, the root view, `poll`, keyboard shortcuts and headless scenes.

use krampsta_agent::{
    Account, DeviceCode, Effort, EffortLevel, KeySource, MockCatalog, Provider, ProviderKind, Selection,
};
use krampsta_core::{AgentEvent, AgentStatus, MessageRole, RunId, ToolCall, ToolCallId, ToolResult, Usage};
use rune::{AppSpec, Chrome, Cx, El, Key, KeyEvent, PointerPress, TextEdit, Tx, Waker, row};
use rune_kit::shell::{self, SidebarToggle, SplitView, Toolbar};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit};

use crate::accounts::SignIn;
use crate::components::{icons, menus, sign_in, status};
use crate::keys;
use crate::menus::MenuTarget;
use crate::picker::{Models, Picker};
use crate::state::{KrampstaApp, Screen};
use crate::theme::{self, ThemeChoice};
use crate::views::{inspector, settings, sidebar};

pub const TITLE: &str = "Krampsta";
pub const SIZE: (f32, f32) = (1200.0, 800.0);
pub const MIN_SIZE: (f32, f32) = (760.0, 520.0);

/// The app as Rune runs it. `init` builds the default (mock) app; `krampsta-app` replaces it
/// with `App::init` to choose the engine.
pub fn spec() -> AppSpec<KrampstaApp> {
    AppSpec {
        min_size: Some(MIN_SIZE),
        scenes: SCENES,
        fonts: theme::FONTS,
        families: theme::FAMILIES,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new(TITLE, SIZE, KrampstaApp::with_mock, view)
    }
}

/// Window chrome: content under a transparent title bar on macOS (the toolbar draws it), with
/// the traffic lights centred on the toolbar. Other platforms keep their native frame.
pub fn chrome() -> Chrome {
    Chrome { full_size_content: true, traffic_lights: Some((16.0, 25.0)), drag_regions: true }
}

fn setup(s: &mut KrampstaApp, waker: Waker) {
    // The saved sessions are read off the UI thread and land in `poll`: the first frame never
    // waits on the disk.
    s.load_history(waker.clone());
    s.set_notifier(waker);
    // The selected provider's models load in the background while the window opens.
    s.ensure_models();
}

/// Runs before every frame and whenever the agent host wakes the window.
pub fn poll(s: &mut KrampstaApp, clock: f64, tx: &mut Tx) -> bool {
    let landed = s.pump();
    // Settings that changed are saved once they settle (nothing visible changes).
    s.sync_prefs(clock, tx);
    // Then one frame's worth of the streaming reply (asks for the next frame while any waits).
    s.step_pace(clock, tx) | landed
}

pub fn on_key(s: &mut KrampstaApp, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key == Key::Escape && s.sign_in != SignIn::Closed {
        s.close_sign_in();
        return true;
    }
    if ev.key == Key::Escape && s.engine.open.is_some() {
        s.close_picker();
        return true;
    }
    if shell::is_sidebar_toggle(ev) {
        s.sidebar.toggle();
        return true;
    }
    if ev.is_command('n') && !ev.mods.shift {
        s.new_session();
        return true;
    }
    if ev.is_command('\\') && s.screen == Screen::Chat {
        s.toggle_split();
        return true;
    }
    if ev.is_command('w') && !ev.mods.shift && s.screen == Screen::Chat {
        s.close_tab(s.selected);
        return true;
    }
    if ev.is_command('o') && !ev.mods.shift {
        s.choose_project(tx);
        return true;
    }
    if ev.is_command('j') {
        s.toggle_changes();
        return true;
    }
    if ev.is_command(',') {
        s.open_settings();
        return true;
    }
    if ev.key == Key::Escape && s.screen == Screen::Settings {
        s.screen = Screen::Chat;
        return true;
    }
    false
}

pub fn view(s: &KrampstaApp, cx: &mut Cx<KrampstaApp>) -> El {
    let k = theme::kit(s.theme.is_dark(cx.system_dark()));
    // Open listboxes (the pickers) mount above everything else.
    let mut layer = Vec::new();
    let screen = match s.screen {
        Screen::Settings => settings::view(s, cx, k, &mut layer),
        Screen::Chat => workspace(s, cx, k, &mut layer),
    };
    sign_in::dialog(s, k, &mut layer);
    menus::delete_dialog(s, k, &mut layer);
    crate::approvals::dialog(s, k, &mut layer);
    // The root's rect is the window: refit the side panes whenever its width changes.
    let base = rune::el()
        .key("krampsta-root")
        .w_full()
        .h_full()
        .on_layout(|s: &mut KrampstaApp, r: [f32; 4]| s.fit_width(r[2]))
        .child(screen);
    rune_kit::root(base, layer)
}

fn workspace(s: &KrampstaApp, cx: &mut Cx<KrampstaApp>, k: &Kit, layer: &mut Vec<El>) -> El {
    let session = s.current();
    let status = session.map(|x| x.status.clone()).unwrap_or_default();
    let open = s.changes.is_open();
    let toggle = IconButton::new(&shell::INSPECTOR)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(open)
        .label(if open { "Hide changes" } else { "Show changes" })
        .help(if cfg!(target_os = "macos") { "Changes (⌘J)" } else { "Changes (Ctrl+J)" })
        .on_press(|s: &mut KrampstaApp| s.toggle_changes())
        .build(k)
        .key(keys::CHANGES_TOGGLE);
    let split = s.split.is_some();
    let split_toggle = IconButton::new(&icons::SPLIT)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(split)
        .label(if split { "Close the split" } else { "Split the conversation" })
        .help(if cfg!(target_os = "macos") { "Side by side (⌘\\)" } else { "Side by side (Ctrl+\\)" })
        .on_press(|s: &mut KrampstaApp| s.toggle_split())
        .build(k)
        .key(keys::SPLIT_TOGGLE);
    // The open sessions' tabs fill the leading side (the active one names the session) and
    // scroll when they don't fit, so they never run under the pill.
    let toolbar = Toolbar::new()
        .leading(row().shrink().min_w(0.0).child(crate::tabs::strip(s, cx, k, layer)))
        .trailing(row().gap(8.0).items_center().child(status::pill(&status, k)).child(split_toggle).child(toggle));
    SplitView::new(crate::panes::view(s, k, layer))
        .sidebar(&s.sidebar, |s: &mut KrampstaApp| &mut s.sidebar, sidebar::view(s, k, layer))
        .inspector(&s.changes, |s: &mut KrampstaApp| &mut s.changes, inspector::view(s, k))
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(14.0, 10.0)
        .sidebar_toggle(SidebarToggle::new().size(ControlSize::Large).past_lights(16.0).edge(18.0).gap(4.0).in_header())
        .reduced_motion(cx.is_reduced_motion())
        .build(k)
}

// ---------------------------------------------------------------------------------------------
// Headless scenes (`--scene NAME`, `--list-scenes`): each stages a state without running the
// agent, so snapshots are deterministic.

type Scene = (&'static str, fn(&mut KrampstaApp));

const SCENES: &[Scene] = &[
    ("light", |s| s.theme = ThemeChoice::Light),
    ("empty", |s| {
        s.new_session();
    }),
    ("draft", |s| {
        s.new_session();
        s.set_draft("Explain how sessions and runs fit together");
    }),
    ("running", scene_running),
    ("tools", scene_tools),
    ("thinking", |s| {
        let run = staged(s, "Why does the retry loop give up early?");
        apply(s, &[AgentEvent::ReasoningDelta { run, text: "**Reading the retry loop**\n\nThe".into() }]);
    }),
    ("reasoning", scene_reasoning),
    ("completed", scene_completed),
    ("failed", scene_failed),
    ("cancelled", |s| {
        let run = staged(s, "Rewrite the build script");
        apply(s, &[AgentEvent::Cancelled { run }]);
    }),
    ("settings", |s| {
        stage_accounts(s, false);
        s.open_settings();
    }),
    // A key typed into the masked field, ready to save.
    ("api-key", |s| {
        stage_accounts(s, false);
        s.open_settings();
        s.api_key.draft = TextEdit::new("sk-proj-0123456789").secure();
    }),
    ("api-key-saved", |s| {
        stage_saved_key(s);
        s.open_settings();
    }),
    ("signed-in", |s| {
        stage_accounts(s, true);
        s.open_settings();
    }),
    ("sign-in", |s| {
        stage_accounts(s, false);
        s.open_settings();
        let code = DeviceCode { url: "https://github.com/login/device".into(), code: "ABCD-1234".into() };
        s.sign_in = SignIn::Code { provider: "copilot".into(), code };
    }),
    ("sign-in-failed", |s| {
        stage_accounts(s, false);
        s.open_settings();
        let error = "The code expired before it was entered. Ask for a new one and enter it within 15 minutes.";
        s.sign_in = SignIn::Failed { provider: "chatgpt".into(), error: error.into() };
    }),
    ("changes-hidden", |s| s.toggle_changes()),
    // Right-clicked: the first session's row, the agent's reply, then deleting that session.
    ("session-menu", |s| {
        let id = s.sessions[0].id;
        s.open_menu(MenuTarget::Session(id), PointerPress { x: 150.0, y: 175.0 });
    }),
    ("message-menu", |s| {
        scene_completed(s);
        let reply = s.current().and_then(|x| x.messages.last()).map(|m| m.id);
        if let Some(id) = reply {
            s.open_menu(MenuTarget::Message(id), PointerPress { x: 420.0, y: 245.0 });
        }
    }),
    ("delete-session", |s| {
        let id = s.sessions[0].id;
        s.ask_delete(id);
    }),
    // Nothing proposed yet: the changes panel's empty state beside a new session.
    ("changes-empty", |s| {
        s.new_session();
    }),
    // The agent proposed edits to two files and a new one: each file and each block can be
    // accepted or rejected.
    ("changes", stage_changes),
    ("command", |s| {
        scene_command(s);
    }),
    ("approval", scene_approval),
    ("approval-long", scene_approval_long),
    ("approval-apply", scene_approval_apply),
    // A tool from an MCP server asks too, worded for a call rather than a command.
    ("approval-mcp", scene_approval_mcp),
    ("approval-applying", scene_approval_applying),
    ("streaming", scene_streaming),
    ("change-failed", |s| {
        stage_changes(s);
        s.review.error = Some(
            "`src/net/backoff.rs` changed on disk after Krampsta proposed this edit, so it was not \
             written; reject it and ask again"
                .into(),
        );
    }),
    ("sidebar-hidden", |s| s.sidebar.toggle()),
    // Signed in and on a real provider: the list shows its groups, the simulated agent hidden.
    ("provider-picker", |s| {
        s.new_session();
        stage_accounts(s, true);
        stage_local(s, Models::Ready(vec!["qwen3:4b".into()]));
        s.engine.selection.model = Some("qwen3:4b".into());
        s.engine.open = Some(Picker::ComposerProvider);
    }),
    // ChatGPT takes a reasoning effort: its picker joins the others with the model's levels,
    // opened on "Extra high".
    ("effort-picker", |s| {
        s.new_session();
        stage_accounts(s, true);
        s.engine.selection = Selection { provider: "chatgpt".into(), model: Some("gpt-6.2-sol".into()) };
        s.engine.models.insert("chatgpt".into(), Models::Ready(vec!["gpt-6.2-sol".into()]));
        s.engine.efforts = scene_efforts();
        s.engine.effort = Some(Effort::new("xhigh"));
        s.engine.open = Some(Picker::ComposerEffort);
    }),
    ("model-picker", |s| {
        stage_providers(s);
        stage_local(s, Models::Ready(vec!["gemma3:12b".into(), "llama3.2:3b".into(), "qwen3:4b".into()]));
        s.engine.selection.model = Some("qwen3:4b".into());
        s.engine.open = Some(Picker::ComposerModel);
    }),
    ("model-error", |s| {
        stage_providers(s);
        stage_local(
            s,
            Models::Failed("could not reach http://localhost:11434/v1/models: error sending request".into()),
        );
        s.engine.problem = Some("Choose a model for Ollama.".into());
        s.engine.open = Some(Picker::ComposerModel);
    }),
    ("projects", scene_projects),
    ("edit", scene_edit),
    // A message that carries a screenshot, and another attached to the draft.
    ("image", crate::images::scene),
    // Two sessions side by side: a finished conversation (active) and the welcome beside it.
    ("split", |s| {
        scene_completed(s);
        s.toggle_split();
    }),
    // A first launch: no history yet, so one new, empty session and no examples.
    ("fresh", |s| {
        s.sessions.clear();
        s.new_session();
    }),
    ("cost", scene_cost),
    ("prices", scene_prices),
    // Settings with an MCP server in each state: connected with its tools, connecting, failed, off.
    // The right panel's "Files" view: the project's tree, filtered by nothing, with proposed files marked.
    ("files", crate::files::scene),
    ("mcp-servers", |s| {
        // Settings shows the providers too: the stills assign them, never the machine's own.
        stage_accounts(s, false);
        crate::servers::scene(s);
    }),
    // The form that adds a server, filled in, and a server waiting for the click that removes it.
    ("mcp-add", |s| {
        stage_accounts(s, false);
        crate::servers::scene_add(s);
    }),
    // No folder open: the row invites opening one.
    ("no-project", |s| {
        s.project = None;
        s.opening_project = None;
    }),
    ("opening-project", |s| {
        s.project = Some(crate::sample::project());
        s.opening_project = Some("/Users/you/code/rune".into());
    }),
];

const SCENE_RUN: RunId = RunId(10_000);

/// A new session whose run has just started.
fn staged(s: &mut KrampstaApp, prompt: &str) -> RunId {
    s.new_session();
    if let Some(x) = s.sessions.first_mut() {
        // The scene's prompt is never blank and the session is new, so this can't fail.
        if x.submit(prompt, SCENE_RUN).is_ok() {
            x.apply(&AgentEvent::Started { run: SCENE_RUN });
        }
    }
    SCENE_RUN
}

fn apply(s: &mut KrampstaApp, events: &[AgentEvent]) {
    if let Some(x) = s.sessions.first_mut() {
        for e in events {
            x.apply(e);
        }
    }
}

fn tool(run: RunId) -> (AgentEvent, AgentEvent) {
    let id = ToolCallId("scene-call".into());
    let call = ToolCall { id: id.clone(), name: "list_files".into(), arguments: r#"{"path": "."}"#.into() };
    let result = ToolResult { call_id: id, output: "Cargo.toml\ncrates/".into(), is_error: false };
    (AgentEvent::ToolCallStarted { run, call }, AgentEvent::ToolCallFinished { run, result })
}

/// An `edit_file` call whose proposed change shows in its card, and in the changes panel.
fn scene_edit(s: &mut KrampstaApp) {
    let run = staged(s, "Add jitter to the retry backoff");
    let id = ToolCallId("scene-edit".into());
    let arguments = r#"{"path": "src/net/backoff.rs"}"#.into();
    let call = ToolCall { id: id.clone(), name: "edit_file".into(), arguments };
    let output = "Proposed a change to src/net/backoff.rs for review.".into();
    let result = ToolResult { call_id: id.clone(), output, is_error: false };
    let reply = "Each wait now spreads by ±20%, so clients that failed together retry apart.";
    apply(
        s,
        &[
            AgentEvent::ToolCallStarted { run, call },
            AgentEvent::ToolCallFinished { run, result },
            AgentEvent::TextDelta { run, text: reply.into() },
            AgentEvent::Completed { run },
        ],
    );
    if let Some(change) = crate::sample::changes().into_iter().next() {
        // What `pump` does with the call's `ChangeProposed`.
        s.card_diffs.insert(id, crate::changes::diff_of(&change));
        s.propose(change);
    }
}

fn scene_running(s: &mut KrampstaApp) {
    let run = staged(s, "Find where the configuration is loaded");
    let (started, _) = tool(run);
    apply(s, &[started, AgentEvent::TextDelta { run, text: "Looking through the workspace for".into() }]);
    // The text so far shows, as it would once paced out.
    s.catch_up();
}

/// One tool card of each kind while the run goes on: done and opened, failed, still running.
fn scene_tools(s: &mut KrampstaApp) {
    let run = staged(s, "Fix the failing checkout test");
    let call = |id: &str, name: &str, arguments: &str| ToolCall {
        id: ToolCallId(id.into()),
        name: name.into(),
        arguments: arguments.into(),
    };
    let result = |id: &str, output: &str, is_error: bool| ToolResult {
        call_id: ToolCallId(id.into()),
        output: output.into(),
        is_error,
    };
    let read = call("scene-read", "read_file", r#"{"path": "src/checkout.rs"}"#);
    let bash = call("scene-bash", "bash", r#"{"command": "cargo test checkout"}"#);
    let search = call("scene-search", "search", r#"{"pattern": "fn total", "path": "src"}"#);
    let source = "pub fn total(items: &[Item]) -> u32 {\n    items.iter().map(|i| i.price).sum()\n}";
    apply(
        s,
        &[
            AgentEvent::ToolCallStarted { run, call: read.clone() },
            AgentEvent::ToolCallFinished { run, result: result("scene-read", source, false) },
            AgentEvent::ToolCallStarted { run, call: bash },
            AgentEvent::ToolCallFinished {
                run,
                result: result("scene-bash", "error: no `cargo` found in PATH\nexit status 127", true),
            },
            AgentEvent::ToolCallStarted { run, call: search },
        ],
    );
    s.open_tools.insert(read.id);
}

/// A finished reply with its reasoning opened: a two-part summary, as OpenAI's models send it.
fn scene_reasoning(s: &mut KrampstaApp) {
    let run = staged(s, "Why does the retry loop give up early?");
    let thought = "**Reading the retry loop**\n\nThe loop stops when `attempt` reaches `max_attempts`, \
                   but `attempt` starts at 1, so it makes one try fewer than configured.\n\n\
                   **Checking the tests**\n\nNo test covers the last attempt, which is why it slipped through.";
    let reply = "It gives up one try early: `attempt` starts at **1**, so `attempt < max_attempts` stops \
                 a step too soon. Start it at 0, or compare with `<=`.";
    apply(
        s,
        &[
            AgentEvent::ReasoningDelta { run, text: thought.into() },
            AgentEvent::TextDelta { run, text: reply.into() },
            AgentEvent::Completed { run },
        ],
    );
    if let Some(id) = s.current().and_then(|x| x.messages.last()).map(|m| m.id) {
        s.open_reasoning.insert(id);
    }
}

/// Efforts as ChatGPT's catalog announces them for a model, with what each is for.
fn scene_efforts() -> Vec<EffortLevel> {
    [
        ("low", "Fast responses with lighter reasoning"),
        ("medium", "Balances speed and reasoning depth"),
        ("high", "Greater reasoning depth for complex problems"),
        ("xhigh", "Extra deep reasoning for the hardest problems"),
    ]
    .map(|(id, about)| EffortLevel { effort: Effort::new(id), description: Some(about.to_owned()) })
    .into()
}

/// Sessions from three projects: the open one first, another with two sessions, and a third
/// folded away.
fn scene_projects(s: &mut KrampstaApp) {
    let open = crate::sample::project();
    s.project = Some(open.clone());
    for x in &mut s.sessions {
        x.project = Some(open.root.clone());
    }
    let other = |s: &mut KrampstaApp, root: &str, title: &str, reply: &str| {
        let id = s.new_session();
        if let Some(x) = s.sessions.iter_mut().find(|x| x.id == id) {
            x.project = Some(root.into());
            x.title = title.into();
            x.push_message(MessageRole::User, title);
            x.push_message(MessageRole::Assistant, reply);
            x.status = AgentStatus::Completed;
        }
    };
    other(s, "/Users/you/code/rune", "Fade the scroll bar at rest", "The bar now fades after 800 ms.");
    other(s, "/Users/you/code/rugents", "Bump the theme crate", "Updated to the new revision.");
    other(s, "/Users/you/code/rune", "Why does the tab strip jitter?", "Its width rounds every frame.");
    s.collapsed_groups = std::iter::once(Some("/Users/you/code/rugents".into())).collect();
    // The first sample stays selected, as at launch.
    if let Some(first) = s.sessions.iter().find(|x| x.project.as_deref() == Some(open.root.as_path())).map(|x| x.id) {
        s.selected = first;
        s.focus_composer(first);
    }
    s.notice = None;
}

fn scene_completed(s: &mut KrampstaApp) {
    let run = staged(s, "Find where the configuration is loaded");
    let (started, finished) = tool(run);
    let reply = "Configuration is loaded in `crates/krampsta-app/src/main.rs`, which builds the engine \
                 and hands it to the UI:\n\n```rust\nlet engine = Arc::new(MockAgent::new(MockConfig::default()));\n\
                 rune::App::new(krampsta_ui::spec()).init(|| KrampstaApp::new(engine)).run();\n```\n\n\
                 Swap the engine there to use a real provider.";
    // Counted as a provider reports it, so the toolbar shows the session's tokens.
    let usage = AgentEvent::Usage { run, usage: Usage { input: 12_480, output: 342 } };
    apply(
        s,
        &[started, finished, AgentEvent::TextDelta { run, text: reply.into() }, usage, AgentEvent::Completed { run }],
    );
}

/// `completed`, run with a priced OpenAI model: the toolbar shows what it cost.
fn scene_cost(s: &mut KrampstaApp) {
    stage_saved_key(s);
    s.engine.selection = Selection { provider: "openai".into(), model: Some("gpt-5.1".into()) };
    s.engine.problem = None;
    s.engine.models.insert("openai".into(), Models::Ready(vec!["gpt-5.1".into(), "gpt-5.1-mini".into()]));
    s.prices.insert(("openai".into(), "gpt-5.1".into()), crate::costs::Price { input: 1.25, output: 10.0 });
    let run = staged(s, "Find where the configuration is loaded");
    if let Some(x) = s.sessions.iter_mut().find(|x| x.active_run() == Some(run)) {
        x.bill_to("openai", "gpt-5.1");
    }
    let reply = "It's loaded in `crates/krampsta-app/src/main.rs`, before the first frame.";
    let usage = AgentEvent::Usage { run, usage: Usage { input: 182_480, output: 6_342 } };
    apply(s, &[AgentEvent::TextDelta { run, text: reply.into() }, usage, AgentEvent::Completed { run }]);
}

/// Settings with two billed models: one priced, one not yet.
fn scene_prices(s: &mut KrampstaApp) {
    stage_saved_key(s);
    s.engine.selection = Selection { provider: "openai".into(), model: Some("gpt-5.1-mini".into()) };
    s.engine.problem = None;
    s.engine.models.insert("openai".into(), Models::Ready(vec!["gpt-5.1".into(), "gpt-5.1-mini".into()]));
    s.prices.insert(("openai".into(), "gpt-5.1".into()), crate::costs::Price { input: 1.25, output: 10.0 });
    s.open_settings();
}

fn scene_failed(s: &mut KrampstaApp) {
    let run = staged(s, "/fail deploy to production");
    apply(
        s,
        &[AgentEvent::Failed { run, error: "provider error: simulated failure (you asked for it with /fail)".into() }],
    );
}

/// The providers of an environment without keys and signed in to no account, whatever the
/// real environment and keychain hold, so the snapshot is the same on every machine.
fn stage_providers(s: &mut KrampstaApp) {
    s.new_session();
    stage_accounts(s, false);
}

/// [`stage_providers`]'s list, with the ChatGPT and Copilot accounts signed in or not.
fn stage_accounts(s: &mut KrampstaApp, signed_in: bool) {
    let provider = |id: &str, label: &str, missing: Option<&str>, kind: ProviderKind| Provider {
        id: id.into(),
        label: label.into(),
        default_model: None,
        missing: missing.map(str::to_owned),
        kind,
        reasoning_effort: false,
        images: false,
        api_key: None,
    };
    let local = |id: &str, label: &str| provider(id, label, None, ProviderKind::Local);
    let account = |id: &str, label: &str, user: &str| {
        let state = if signed_in { Account::SignedIn { user: user.into() } } else { Account::SignedOut };
        let missing = (!signed_in).then_some("needs signing in (Settings › Accounts)");
        // As in the real catalog, ChatGPT runs take a reasoning effort.
        Provider { reasoning_effort: id == "chatgpt", ..provider(id, label, missing, ProviderKind::Account(state)) }
    };
    s.engine.providers = vec![
        Provider {
            api_key: Some(KeySource::Missing),
            ..provider("openai", "OpenAI", Some(OPENAI_KEY_HINT), ProviderKind::Cloud)
        },
        account("chatgpt", "ChatGPT", "me@example.com"),
        account("copilot", "GitHub Copilot", "octocat"),
        local("llamacpp", "llama.cpp"),
        local("ollama", "Ollama"),
        local("lmstudio", "LM Studio"),
        MockCatalog::provider(),
    ];
}

/// What the real catalog says while OpenAI has no key.
const OPENAI_KEY_HINT: &str = "needs an API key (Settings › API keys, or OPENAI_API_KEY)";

/// [`stage_accounts`]'s list with an OpenAI key saved in the app.
fn stage_saved_key(s: &mut KrampstaApp) {
    stage_accounts(s, false);
    if let Some(openai) = s.engine.providers.iter_mut().find(|p| p.id == "openai") {
        openai.api_key = Some(KeySource::Saved);
        openai.missing = None;
    }
}

/// Ollama selected, its model list in `models`.
fn stage_local(s: &mut KrampstaApp, models: Models) {
    s.engine.selection = Selection { provider: "ollama".into(), model: None };
    s.engine.models.insert("ollama".into(), models);
}

/// A new session beside the sample proposals, as the agent's edits arrive.
fn stage_changes(s: &mut KrampstaApp) {
    s.new_session();
    for change in crate::sample::changes() {
        s.propose(change);
    }
}

/// A command running, its last lines showing as they come.
fn scene_command(s: &mut KrampstaApp) -> ToolCall {
    let run = staged(s, "Run the tests");
    let call = ToolCall {
        id: ToolCallId("scene-command".into()),
        name: "run_command".into(),
        arguments: r#"{"command": "cargo test --workspace"}"#.into(),
    };
    let output = "   Compiling krampsta-core v0.1.0\n   Compiling krampsta-agent v0.1.0\n   Compiling krampsta-tools v0.1.0\n\
                  \x20   Finished `test` profile in 14.2s\n     Running unittests src/lib.rs\n\nrunning 25 tests\n\
                  test policy::tests::rules_allow_plain_commands_and_ask_about_the_rest ... ok\n";
    apply(
        s,
        &[
            AgentEvent::ToolCallStarted { run, call: call.clone() },
            AgentEvent::ToolOutputDelta { run, call: call.id.clone(), text: output.into() },
        ],
    );
    call
}

/// A command waiting for the user: the dialog over the call's card.
fn scene_approval(s: &mut KrampstaApp) {
    let run = staged(s, "Clean the build and run the tests");
    let call = ToolCall {
        id: ToolCallId("scene-approval".into()),
        name: "run_command".into(),
        arguments: r#"{"command": "cargo clean && cargo test --workspace"}"#.into(),
    };
    apply(s, &[AgentEvent::ToolCallStarted { run, call: call.clone() }]);
    s.ask(crate::approvals::Asking {
        run,
        call: call.id,
        tool: call.name,
        what: "cargo clean && cargo test --workspace".into(),
    });
}

fn scene_approval_mcp(s: &mut KrampstaApp) {
    let run = staged(s, "Open an issue for the flaky retry test");
    let arguments = r#"{"repo": "you/krampsta", "title": "Retry test fails one run in twenty"}"#;
    let call = ToolCall {
        id: ToolCallId("scene-approval-mcp".into()),
        name: "mcp__github__create_issue".into(),
        arguments: arguments.into(),
    };
    apply(s, &[AgentEvent::ToolCallStarted { run, call: call.clone() }]);
    // As `ToolSet::check` words it: the tool, its server and the arguments, pretty.
    let what = "create_issue on the MCP server `github`
{
  \"repo\": \"you/krampsta\",
  \"title\": \"Retry test fails one run in twenty\"
}";
    s.ask(crate::approvals::Asking { run, call: call.id, tool: call.name, what: what.into() });
}

fn scene_approval_apply(s: &mut KrampstaApp) {
    scene_approval(s);
    s.propose(krampsta_core::FileChange {
        path: "README.md".into(),
        before: Some("# Example project\n".into()),
        after: "# Example project\n\nRun `cargo test` to validate changes.\n".into(),
    });
}

fn scene_approval_applying(s: &mut KrampstaApp) {
    scene_approval_apply(s);
    s.stage_applying_scene();
}

/// A reply that has just arrived in one burst, not yet shown: the pacer lets it out over the
/// next frames (`--frames`/`--strip` show how it comes out).
fn scene_streaming(s: &mut KrampstaApp) {
    let run = staged(s, "Explain how a run streams into the transcript");
    let reply = "A run streams its reply as events. The engine emits text deltas through its context, \
                 the host sends them down a channel and wakes the window, and `poll` folds them into the \
                 session. The view then shows the reply at a steady pace instead of in the bursts the \
                 provider sends, a whole word at a time, each fading in as it comes out.";
    apply(s, &[AgentEvent::TextDelta { run, text: reply.into() }]);
}

/// A command that is a whole script: it scrolls inside the dialog, the buttons stay on screen.
fn scene_approval_long(s: &mut KrampstaApp) {
    let run = staged(s, "Rename the helper everywhere");
    let what: String =
        (1..=80).map(|n| format!("edit('src/file{n}.rs', lambda s: s.replace('old_name', 'new_name'))\n")).collect();
    let call = ToolCall {
        id: ToolCallId("scene-approval-long".into()),
        name: "run_command".into(),
        arguments: serde_json::json!({ "command": what }).to_string(),
    };
    apply(s, &[AgentEvent::ToolCallStarted { run, call: call.clone() }]);
    s.ask(crate::approvals::Asking { run, call: call.id, tool: call.name, what });
}
