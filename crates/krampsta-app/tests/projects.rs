//! Opening another project folder, end to end: the project row pressed in Rune's headless
//! runtime, the folder dialog answered, the folder opened by the real tools off the UI thread,
//! and the agent's next run reading files from the new folder.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use krampsta_agent::{
    AgentEngine, AgentError, BoxFuture, EngineCatalog, MockCatalog, OpenedProject, ProjectOpener, Provider, RunContext,
    RunRequest, Selection,
};
use krampsta_core::{AgentStatus, ToolCall, ToolCallId};
use krampsta_tools::Workspace;
use krampsta_ui::{KrampstaApp, app, keys};
use rune::{Env, FileDialogKind, Runtime, TextEdit};

/// Reads `here.txt` from the project and says what it holds.
struct ReadsHere;

impl AgentEngine for ReadsHere {
    fn name(&self) -> &str {
        "reads-here"
    }
    fn run(self: Arc<Self>, _: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
        Box::pin(async move {
            let call = ToolCall {
                id: ToolCallId("read".into()),
                name: "read_file".into(),
                arguments: r#"{"path":"here.txt"}"#.into(),
            };
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
        Ok(Arc::new(ReadsHere))
    }
    fn models(&self, provider: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
        MockCatalog::default().models(provider)
    }
}

/// The real tools, with their rules and log in a temporary config folder.
struct Opens(PathBuf);

impl ProjectOpener for Opens {
    fn open(&self, root: &Path) -> Result<OpenedProject, String> {
        Workspace::open_with(root, Some(self.0.clone())).map(Workspace::into_project).map_err(|e| e.to_string())
    }
}

fn folder(says: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("here.txt"), says).unwrap();
    dir
}

fn pump_until(rt: &mut Runtime<KrampstaApp>, what: &str, done: impl Fn(&KrampstaApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(&rt.state) {
        assert!(Instant::now() < deadline, "{what} never happened; notice: {:?}", rt.state.notice);
        rt.update_if(|s, _| s.pump());
        rt.frame(1.0 / 60.0);
        std::thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
}

fn label(rt: &Runtime<KrampstaApp>, text: &str) -> bool {
    rt.elements().iter().any(|e| !e.exiting && e.text.as_deref() == Some(text))
}

#[test]
fn a_folder_picked_in_the_dialog_becomes_the_project_the_agent_reads() {
    let (first, second, config) = (folder("first folder"), folder("second folder"), tempfile::tempdir().unwrap());
    let opener = Arc::new(Opens(config.path().to_owned()));
    let start = opener.open(first.path()).unwrap();
    let first_root = start.root.clone();
    let state = KrampstaApp::new(Arc::new(Catalog)).with_project(start).with_opener(opener);
    let mut rt = Runtime::new(state, app::view, (1200.0, 800.0), Env::new(true, 1.0));
    rt.frame(1.0 / 60.0);
    let first_name = rt.state.project.as_ref().unwrap().name.clone();
    assert!(label(&rt, &first_name), "the sidebar names the project");

    let (x, y) = rt.find(keys::PROJECT).expect("the project row");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    let (kind, options) = rt.file_dialog().expect("pressing the row opens the folder dialog");
    assert_eq!(kind, FileDialogKind::Folder);
    assert_eq!(options.directory.as_deref(), Some(first_root.as_path()), "it starts in the open project");

    rt.resolve_file_dialog(Some(second.path().to_owned())).unwrap();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opening_project.as_deref(), Some(second.path()));
    assert!(label(&rt, "Opening…"));
    let second_root = second.path().canonicalize().unwrap();
    pump_until(&mut rt, "the switch", |s| s.project.as_ref().is_some_and(|p| p.root == second_root));
    let second_name = rt.state.project.as_ref().unwrap().name.clone();
    assert!(label(&rt, &second_name), "the sidebar names the new project");

    rt.update(|s, _| {
        s.new_session();
        s.draft = TextEdit::new("what does here.txt say?");
        s.send().unwrap();
    });
    pump_until(&mut rt, "the reply", |s| s.current().is_some_and(|x| x.status == AgentStatus::Completed));
    let reply = rt.state.current().and_then(|s| s.messages.last()).map(|m| m.content.clone()).unwrap();
    assert!(reply.contains("second folder"), "the run read the new folder: {reply}");
}

#[test]
fn cancelling_the_dialog_or_picking_a_file_keeps_the_project() {
    let (first, config) = (folder("first folder"), tempfile::tempdir().unwrap());
    let opener = Arc::new(Opens(config.path().to_owned()));
    let start = opener.open(first.path()).unwrap();
    let root = start.root.clone();
    let state = KrampstaApp::new(Arc::new(Catalog)).with_project(start).with_opener(opener);
    let mut rt = Runtime::new(state, app::view, (1200.0, 800.0), Env::new(true, 1.0));
    rt.frame(1.0 / 60.0);

    rt.update(|s, tx| s.choose_project(tx));
    rt.frame(1.0 / 60.0);
    rt.resolve_file_dialog(None).unwrap();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opening_project, None, "cancelled: nothing opens");

    rt.update(|s, tx| s.choose_project(tx));
    rt.frame(1.0 / 60.0);
    rt.resolve_file_dialog(Some(first.path().join("here.txt"))).unwrap();
    rt.frame(1.0 / 60.0);
    assert!(rt.state.opening_project.is_some(), "the answer runs on the next frame");
    pump_until(&mut rt, "the refusal", |s| s.opening_project.is_none());
    assert_eq!(rt.state.project.as_ref().map(|p| p.root.clone()), Some(root));
    assert!(rt.state.notice.is_some(), "it says why the file can't be the project");
}
