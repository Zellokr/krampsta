//! Rune wiring: the app spec, the root view, `poll`, keyboard shortcuts and headless scenes.

use krampsta_agent::{
    Account, DeviceCode, Effort, EffortLevel, KeySource, MockCatalog, Provider, ProviderKind, Selection,
};
use krampsta_core::{AgentEvent, RunId, ToolCall, ToolCallId, ToolResult, Usage};
use rune::{AppSpec, Chrome, Cx, El, Key, KeyEvent, PointerPress, TextEdit, Tx, Waker, row, text};
use rune_kit::shell::{self, SidebarToggle, SplitView, Toolbar};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit};

use crate::accounts::SignIn;
use crate::components::{menus, sign_in, status};
use crate::keys;
use crate::menus::MenuTarget;
use crate::picker::{Models, Picker};
use crate::state::{KrampstaApp, Screen};
use crate::theme::{self, ThemeChoice};
use crate::views::{changes, chat, settings, sidebar};

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
    s.set_notifier(waker);
    // The selected provider's models load in the background while the window opens.
    s.ensure_models();
}

/// Runs before every frame and whenever the agent host wakes the window.
pub fn poll(s: &mut KrampstaApp, _clock: f64, _tx: &mut Tx) -> bool {
    s.pump()
}

pub fn on_key(s: &mut KrampstaApp, ev: &KeyEvent, _tx: &mut Tx) -> bool {
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
    if ev.is_command('j') {
        s.toggle_changes();
        return true;
    }
    if ev.is_command(',') {
        s.screen = Screen::Settings;
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
    let title = session.map_or(TITLE, |x| x.title.as_str());
    let status = session.map(|x| x.status.clone()).unwrap_or_default();
    let usage = session.and_then(|x| status::usage(x.usage, k));
    let open = s.changes.is_open();
    let toggle = IconButton::new(&shell::INSPECTOR)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(open)
        .label(if open { "Hide changes" } else { "Show changes" })
        .help(if cfg!(target_os = "macos") { "Changes (⌘J)" } else { "Changes (Ctrl+J)" })
        .on_press(|s: &mut KrampstaApp| s.toggle_changes())
        .build(k)
        .key(keys::CHANGES_TOGGLE);
    // The title sits at the leading edge and truncates, so a long one never runs under the pill.
    let heading = text(title.to_owned()).font(k.ty.ui_medium).color(k.c.text_base).truncate().shrink().min_w(0.0);
    let toolbar = Toolbar::new()
        .leading(row().grow(1.0).min_w(0.0).child(heading))
        .trailing(row().gap(8.0).items_center().children(usage).child(status::pill(&status, k)).child(toggle));
    SplitView::new(chat::view(s, k, layer))
        .sidebar(&s.sidebar, |s: &mut KrampstaApp| &mut s.sidebar, sidebar::view(s, k, layer))
        .inspector(&s.changes, |s: &mut KrampstaApp| &mut s.changes, changes::view(s, k))
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
        s.draft = TextEdit::new("Explain how sessions and runs fit together");
        s.draft.focused = true;
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
        s.screen = Screen::Settings;
    }),
    // A key typed into the masked field, ready to save.
    ("api-key", |s| {
        stage_accounts(s, false);
        s.screen = Screen::Settings;
        s.api_key.draft = TextEdit::new("sk-proj-0123456789").secure();
    }),
    ("api-key-saved", |s| {
        stage_saved_key(s);
        s.screen = Screen::Settings;
    }),
    ("signed-in", |s| {
        stage_accounts(s, true);
        s.screen = Screen::Settings;
    }),
    ("sign-in", |s| {
        stage_accounts(s, false);
        s.screen = Screen::Settings;
        let code = DeviceCode { url: "https://github.com/login/device".into(), code: "ABCD-1234".into() };
        s.sign_in = SignIn::Code { provider: "copilot".into(), code };
    }),
    ("sign-in-failed", |s| {
        stage_accounts(s, false);
        s.screen = Screen::Settings;
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

fn scene_running(s: &mut KrampstaApp) {
    let run = staged(s, "Find where the configuration is loaded");
    let (started, _) = tool(run);
    apply(s, &[started, AgentEvent::TextDelta { run, text: "Looking through the workspace for".into() }]);
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
