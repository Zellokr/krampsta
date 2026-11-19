//! Commands waiting for the user to allow them: the queue (several calls may ask at once, one
//! dialog at a time) and the dialog that answers the first.

use krampsta_agent::Decision;
use krampsta_core::{RunId, ToolCallId};
use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Dialog, Kit};

use crate::keys;
use crate::state::KrampstaApp;

/// A call that waits: what it would run, and where to send the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asking {
    pub run: RunId,
    pub call: ToolCallId,
    pub tool: String,
    /// The command line, as it will run.
    pub what: String,
}

impl KrampstaApp {
    pub(crate) fn ask(&mut self, asking: Asking) {
        self.approvals.push_back(asking);
    }

    /// Answers the call the dialog shows, and moves on to the next one waiting.
    pub fn answer(&mut self, decision: Decision) {
        if let Some(a) = self.approvals.pop_front() {
            // False when the run already ended: nothing waits, nothing to do.
            self.agent.answer(a.run, &a.call, decision);
        }
    }

    /// A run ended (or was stopped): its calls no longer wait.
    pub(crate) fn forget_approvals(&mut self, run: RunId) {
        self.approvals.retain(|a| a.run != run);
    }
}

/// The dialog for the first call waiting, if any. Closing it denies, as Escape would.
pub fn dialog(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>) {
    let Some(a) = s.approvals.front() else { return };
    let button = |label: &str, variant, decision, key: &str| {
        Button::new(label)
            .variant(variant)
            .control_size(ControlSize::Regular)
            .on_press(move |s: &mut KrampstaApp| s.answer(decision))
            .build(k)
            .key(key)
    };
    let command = col().w_full().px(12.0).py(10.0).radius(k.radius.sm).fill(k.c.layer_02).child(
        text(a.what.clone())
            .font(k.ty.mono)
            .color(k.c.text_base)
            .wrap_anywhere()
            .selectable(true)
            .selection_color(k.selection()),
    );
    let more = match s.approvals.len() {
        1 => String::new(),
        n => format!(" ({} more waiting)", n - 1),
    };
    let body = col()
        .w_full()
        .gap(14.0)
        .px(16.0)
        .pb(18.0)
        .child(
            text(format!("Krampsta wants to run this in the project folder{more}:"))
                .font(k.ty.ui)
                .color(k.c.text_muted)
                .wrap(),
        )
        .child(command)
        .child(
            row()
                .w_full()
                .gap(8.0)
                .justify_end()
                .child(button("Deny", ButtonVariant::Neutral, Decision::Deny, keys::APPROVAL_DENY))
                .child(button("Always allow", ButtonVariant::Outline, Decision::Always, keys::APPROVAL_ALWAYS))
                .child(button("Allow once", ButtonVariant::Contrast, Decision::Once, keys::APPROVAL_ONCE)),
        );
    Dialog::new(keys::APPROVAL, "Run this command?")
        .open(true)
        .fit()
        .width(520.0)
        .child(body)
        .on_close(|s: &mut KrampstaApp| s.answer(Decision::Deny))
        .build(k, layer);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use krampsta_agent::{
        Access, AgentEngine, AgentError, BoxFuture, Call, EngineCatalog, MockCatalog, Provider, RunContext, RunRequest,
        Selection, ToolOutput, ToolSpec, Toolbox,
    };
    use krampsta_core::{AgentStatus, ToolCall};
    use rune::TextEdit;

    use super::*;

    /// An engine that runs `ls` and writes back what came of it.
    struct RunsLs;

    impl AgentEngine for RunsLs {
        fn name(&self) -> &str {
            "runs-ls"
        }
        fn run(self: Arc<Self>, _: RunRequest, ctx: RunContext) -> BoxFuture<'static, Result<(), AgentError>> {
            Box::pin(async move {
                let call = ToolCall { id: ToolCallId("c".into()), name: "run_command".into(), arguments: "ls".into() };
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
            Ok(Arc::new(RunsLs))
        }
        fn models(&self, p: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
            MockCatalog::default().models(p)
        }
    }

    /// Asks about every command; running one answers "listed".
    struct Asks;

    impl Toolbox for Asks {
        fn specs(&self) -> Arc<[ToolSpec]> {
            Arc::from([])
        }
        fn check(&self, _: &str, arguments: &str) -> Access {
            Access::Ask { what: arguments.to_owned() }
        }
        fn run(&self, _: &str, _: &str, _: &Call<'_>) -> Result<ToolOutput, String> {
            Ok(String::from("listed").into())
        }
    }

    fn send(app: &mut KrampstaApp) {
        app.new_session();
        app.draft = TextEdit::new("list the files");
        app.send().unwrap();
    }

    fn pump_until(app: &mut KrampstaApp, done: impl Fn(&KrampstaApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(app) {
            assert!(Instant::now() < deadline, "timed out");
            app.pump();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn reply(app: &KrampstaApp) -> String {
        app.current().and_then(|s| s.messages.last()).map(|m| m.content.clone()).unwrap_or_default()
    }

    #[test]
    fn the_dialog_queues_what_asks_and_the_answer_lets_it_run() {
        let mut app = KrampstaApp::new(Arc::new(Catalog)).with_tools(Arc::new(Asks));
        send(&mut app);
        pump_until(&mut app, |a| !a.approvals.is_empty());
        assert_eq!(app.approvals.front().map(|a| a.what.as_str()), Some("ls"));
        app.answer(Decision::Once);
        assert!(app.approvals.is_empty());
        pump_until(&mut app, |a| a.current().is_some_and(|s| s.status == AgentStatus::Completed));
        assert_eq!(reply(&app), "listed");
    }

    #[test]
    fn stopping_the_run_takes_its_question_away() {
        let mut app = KrampstaApp::new(Arc::new(Catalog)).with_tools(Arc::new(Asks));
        send(&mut app);
        pump_until(&mut app, |a| !a.approvals.is_empty());
        app.cancel();
        pump_until(&mut app, |a| a.current().is_some_and(|s| s.status == AgentStatus::Cancelled));
        assert!(app.approvals.is_empty(), "nothing waits for an answer any more");
    }
}
