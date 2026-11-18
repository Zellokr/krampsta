//! Rune wiring: the app spec, the root view, `poll`, keyboard shortcuts and headless scenes.

use krune_agent::{Account, DeviceCode, MockCatalog, Provider, Selection};
use krune_core::{AgentEvent, RunId, ToolCall, ToolCallId, ToolResult};
use rune::{AppSpec, Chrome, Cx, El, Key, KeyEvent, TextEdit, Tx, Waker, row, text};
use rune_kit::shell::{self, SidebarToggle, SplitView, Toolbar};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit};

use crate::accounts::SignIn;
use crate::components::{sign_in, status};
use crate::keys;
use crate::picker::{Models, Picker};
use crate::state::{KruneApp, Screen};
use crate::theme::{self, ThemeChoice};
use crate::views::{changes, chat, settings, sidebar};

pub const TITLE: &str = "Krune";
pub const SIZE: (f32, f32) = (1200.0, 800.0);
pub const MIN_SIZE: (f32, f32) = (760.0, 520.0);

/// The app as Rune runs it. `init` builds the default (mock) app; `krune-app` replaces it
/// with `App::init` to choose the engine.
pub fn spec() -> AppSpec<KruneApp> {
    AppSpec {
        min_size: Some(MIN_SIZE),
        scenes: SCENES,
        fonts: theme::FONTS,
        families: theme::FAMILIES,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new(TITLE, SIZE, KruneApp::with_mock, view)
    }
}

/// Window chrome: content under a transparent title bar on macOS (the toolbar draws it), with
/// the traffic lights centred on the toolbar. Other platforms keep their native frame.
pub fn chrome() -> Chrome {
    Chrome { full_size_content: true, traffic_lights: Some((16.0, 25.0)), drag_regions: true }
}

fn setup(s: &mut KruneApp, waker: Waker) {
    s.set_notifier(waker);
    // The selected provider's models load in the background while the window opens.
    s.ensure_models();
}

/// Runs before every frame and whenever the agent host wakes the window.
pub fn poll(s: &mut KruneApp, _clock: f64, _tx: &mut Tx) -> bool {
    s.pump()
}

pub fn on_key(s: &mut KruneApp, ev: &KeyEvent, _tx: &mut Tx) -> bool {
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

pub fn view(s: &KruneApp, cx: &mut Cx<KruneApp>) -> El {
    let k = theme::kit(s.theme.is_dark(cx.system_dark()));
    // Open listboxes (the pickers) mount above everything else.
    let mut layer = Vec::new();
    let screen = match s.screen {
        Screen::Settings => settings::view(s, cx, k, &mut layer),
        Screen::Chat => workspace(s, cx, k, &mut layer),
    };
    sign_in::dialog(s, k, &mut layer);
    // The root's rect is the window: refit the side panes whenever its width changes.
    let base = rune::el()
        .key("krune-root")
        .w_full()
        .h_full()
        .on_layout(|s: &mut KruneApp, r: [f32; 4]| s.fit_width(r[2]))
        .child(screen);
    rune_kit::root(base, layer)
}

fn workspace(s: &KruneApp, cx: &mut Cx<KruneApp>, k: &Kit, layer: &mut Vec<El>) -> El {
    let session = s.current();
    let title = session.map_or(TITLE, |x| x.title.as_str());
    let status = session.map(|x| x.status.clone()).unwrap_or_default();
    let open = s.changes.is_open();
    let toggle = IconButton::new(&shell::INSPECTOR)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(open)
        .label(if open { "Hide changes" } else { "Show changes" })
        .help(if cfg!(target_os = "macos") { "Changes (⌘J)" } else { "Changes (Ctrl+J)" })
        .on_press(|s: &mut KruneApp| s.toggle_changes())
        .build(k)
        .key(keys::CHANGES_TOGGLE);
    // The title sits at the leading edge and truncates, so a long one never runs under the pill.
    let heading = text(title.to_owned()).font(k.ty.ui_medium).color(k.c.text_base).truncate().shrink().min_w(0.0);
    let toolbar = Toolbar::new()
        .leading(row().grow(1.0).min_w(0.0).child(heading))
        .trailing(row().gap(8.0).items_center().child(status::pill(&status, k)).child(toggle));
    SplitView::new(chat::view(s, k, layer))
        .sidebar(&s.sidebar, |s: &mut KruneApp| &mut s.sidebar, sidebar::view(s, k))
        .inspector(&s.changes, |s: &mut KruneApp| &mut s.changes, changes::view(s, k))
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

type Scene = (&'static str, fn(&mut KruneApp));

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
    ("changes-hidden", |s| s.toggle_changes()),
    ("sidebar-hidden", |s| s.sidebar.toggle()),
    ("provider-picker", |s| {
        stage_providers(s);
        s.engine.open = Some(Picker::ComposerProvider);
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
fn staged(s: &mut KruneApp, prompt: &str) -> RunId {
    s.new_session();
    if let Some(x) = s.sessions.first_mut() {
        // The scene's prompt is never blank and the session is new, so this can't fail.
        if x.submit(prompt, SCENE_RUN).is_ok() {
            x.apply(&AgentEvent::Started { run: SCENE_RUN });
        }
    }
    SCENE_RUN
}

fn apply(s: &mut KruneApp, events: &[AgentEvent]) {
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

fn scene_running(s: &mut KruneApp) {
    let run = staged(s, "Find where the configuration is loaded");
    let (started, _) = tool(run);
    apply(s, &[started, AgentEvent::TextDelta { run, text: "Looking through the workspace for".into() }]);
}

fn scene_completed(s: &mut KruneApp) {
    let run = staged(s, "Find where the configuration is loaded");
    let (started, finished) = tool(run);
    let reply = "Configuration is loaded in `crates/krune-app/src/main.rs`, which builds the engine \
                 and hands it to the UI:\n\n```rust\nlet engine = Arc::new(MockAgent::new(MockConfig::default()));\n\
                 rune::App::new(krune_ui::spec()).init(|| KruneApp::new(engine)).run();\n```\n\n\
                 Swap the engine there to use a real provider.";
    apply(s, &[started, finished, AgentEvent::TextDelta { run, text: reply.into() }, AgentEvent::Completed { run }]);
}

fn scene_failed(s: &mut KruneApp) {
    let run = staged(s, "/fail deploy to production");
    apply(
        s,
        &[AgentEvent::Failed { run, error: "provider error: simulated failure (you asked for it with /fail)".into() }],
    );
}

/// The providers of an environment without keys and signed in to no account, whatever the
/// real environment and keychain hold, so the snapshot is the same on every machine.
fn stage_providers(s: &mut KruneApp) {
    s.new_session();
    stage_accounts(s, false);
}

/// [`stage_providers`]'s list, with the ChatGPT and Copilot accounts signed in or not.
fn stage_accounts(s: &mut KruneApp, signed_in: bool) {
    let provider = |id: &str, label: &str, missing: Option<&str>| Provider {
        id: id.into(),
        label: label.into(),
        default_model: None,
        missing: missing.map(str::to_owned),
        account: None,
    };
    let account = |id: &str, label: &str, user: &str| Provider {
        missing: (!signed_in).then(|| "needs signing in (Settings › Accounts)".into()),
        account: Some(if signed_in { Account::SignedIn { user: user.into() } } else { Account::SignedOut }),
        ..provider(id, label, None)
    };
    s.engine.providers = vec![
        provider("openai", "OpenAI", Some("needs OPENAI_API_KEY")),
        account("chatgpt", "ChatGPT", "me@example.com"),
        account("copilot", "GitHub Copilot", "octocat"),
        provider("llamacpp", "llama.cpp", None),
        provider("ollama", "Ollama", None),
        provider("lmstudio", "LM Studio", None),
        MockCatalog::provider(),
    ];
}

/// Ollama selected, its model list in `models`.
fn stage_local(s: &mut KruneApp, models: Models) {
    s.engine.selection = Selection { provider: "ollama".into(), model: None };
    s.engine.models.insert("ollama".into(), models);
}
