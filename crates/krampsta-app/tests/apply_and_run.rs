//! "Apply all changes and run once", end to end: a real proposal from the tools, the real
//! approval dialog pressed in Rune's headless runtime, real writes through the host, and a real
//! shell command that reads the file the change wrote.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use krampsta_agent::{
    AgentEngine, AgentError, Approval, BoxFuture, Call, CancellationToken, EngineCatalog, MockCatalog, Provider,
    RunContext, RunRequest, Selection, Toolbox,
};
use krampsta_core::{AgentStatus, ToolCall, ToolCallId};
use krampsta_tools::Workspace;
use krampsta_ui::{KrampstaApp, app, keys};
use rune::{Env, Runtime, TextEdit};

/// The command the agent runs: print the file the change touches.
const READ: &str = if cfg!(windows) { "Get-Content -Raw greeting.txt" } else { "cat greeting.txt" };

/// Runs `READ` once and writes back what the tool returned.
struct ReadsTheFile;

impl AgentEngine for ReadsTheFile {
    fn name(&self) -> &str {
        "reads-the-file"
    }
    fn run(self: Arc<Self>, _: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
        Box::pin(async move {
            let arguments = serde_json::json!({ "command": READ }).to_string();
            let call = ToolCall { id: ToolCallId("read".into()), name: "run_command".into(), arguments };
            let result = ctx.run_tool(call).await?;
            ctx.text(result.output);
            Ok(())
        })
    }
}

struct Catalog;

impl EngineCatalog for Catalog {
    fn providers(&self) -> Vec<Provider> {
        MockCatalog::default().providers()
    }
    fn initial(&self) -> Selection {
        MockCatalog::default().initial()
    }
    fn engine(&self, _: &str, _: &str) -> Result<Arc<dyn AgentEngine>, AgentError> {
        Ok(Arc::new(ReadsTheFile))
    }
    fn models(&self, provider: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
        MockCatalog::default().models(provider)
    }
}

/// A project with `greeting.txt` saying "old", its tools (rules and log in `config`), the app
/// over them in a headless runtime, and a proposal that changes it to "new".
fn setup(dir: &Path, config: &Path) -> Runtime<KrampstaApp> {
    std::fs::write(dir.join("greeting.txt"), "old\n").unwrap();
    let tools = Arc::new(Workspace::open_with(dir, Some(config.to_owned())).unwrap());
    let cancel = CancellationToken::new();
    let call = Call { cancel: &cancel, approval: &Approval::NotNeeded, output: &|_| {} };
    let edit = r#"{"path":"greeting.txt","old_string":"old","new_string":"new"}"#;
    let change = tools.run("edit_file", edit, &call).unwrap().change.unwrap();

    let state = KrampstaApp::new(Arc::new(Catalog)).with_tools(tools);
    let mut rt = Runtime::new(state, app::view, (1200.0, 800.0), Env::new(true, 1.0));
    rt.update(|s, _| {
        s.propose(change);
        s.new_session();
        s.draft = TextEdit::new("show me the greeting");
        s.send().unwrap();
    });
    rt
}

/// Pumps the host into the app, frame by frame, until `done`.
fn pump_until(rt: &mut Runtime<KrampstaApp>, done: impl Fn(&KrampstaApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(&rt.state) {
        assert!(Instant::now() < deadline, "timed out; approvals: {:?}", rt.state.approvals);
        rt.update_if(|s, _| s.pump());
        rt.frame(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
}

fn click(rt: &mut Runtime<KrampstaApp>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn status(rt: &Runtime<KrampstaApp>) -> Option<AgentStatus> {
    rt.state.current().map(|s| s.status.clone())
}

fn reply(rt: &Runtime<KrampstaApp>) -> String {
    rt.state.current().and_then(|s| s.messages.last()).map(|m| m.content.clone()).unwrap_or_default()
}

#[test]
fn applying_writes_every_change_before_the_command_runs_and_the_command_reads_them() {
    let (dir, config) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut rt = setup(dir.path(), config.path());
    pump_until(&mut rt, |s| !s.approvals.is_empty());
    assert_eq!(std::fs::read_to_string(dir.path().join("greeting.txt")).unwrap(), "old\n", "nothing written yet");

    click(&mut rt, keys::APPROVAL_APPLY_AND_VALIDATE);
    pump_until(&mut rt, |s| s.current().is_some_and(|x| !x.status.is_running()));

    assert_eq!(status(&rt), Some(AgentStatus::Completed));
    assert_eq!(std::fs::read_to_string(dir.path().join("greeting.txt")).unwrap(), "new\n", "the change landed");
    let output = reply(&rt);
    assert!(output.contains("new") && !output.contains("old"), "the command read the written file: {output}");
    assert!(rt.state.review.proposals().is_empty(), "nothing left to review");
    assert!(rt.state.approvals.is_empty());
    let log = std::fs::read_to_string(config.path().join("commands.log")).unwrap();
    assert_eq!(log.lines().count(), 1, "the command ran once: {log}");
    assert!(log.contains(r#""allowed_by":"the user, once""#) && log.contains("exit code 0"), "{log}");
}

#[test]
fn a_write_that_fails_keeps_the_command_from_running() {
    let (dir, config) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut rt = setup(dir.path(), config.path());
    pump_until(&mut rt, |s| !s.approvals.is_empty());
    // Someone edits the file after the proposal: its write must refuse, and the command wait.
    std::fs::write(dir.path().join("greeting.txt"), "edited by hand\n").unwrap();

    click(&mut rt, keys::APPROVAL_APPLY_AND_VALIDATE);
    pump_until(&mut rt, |s| s.review.error.is_some());
    // Give a wrongly released command time to run, then check it didn't.
    for _ in 0..60 {
        rt.update_if(|s, _| s.pump());
        rt.frame(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read_to_string(dir.path().join("greeting.txt")).unwrap(), "edited by hand\n");
    assert!(
        rt.state.review.error.as_deref().is_some_and(|e| e.contains("changed on disk")),
        "{:?}",
        rt.state.review.error
    );
    assert_eq!(rt.state.approvals.len(), 1, "the command still waits for an answer");
    assert_eq!(status(&rt), Some(AgentStatus::Running));
    assert!(!config.path().join("commands.log").exists(), "nothing ran");

    click(&mut rt, keys::APPROVAL_DENY);
    pump_until(&mut rt, |s| s.current().is_some_and(|x| !x.status.is_running()));
    assert!(reply(&rt).starts_with("The user did not allow this"), "{}", reply(&rt));
    assert!(!config.path().join("commands.log").exists(), "denied: still nothing ran");
}
