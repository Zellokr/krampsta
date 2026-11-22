//! MCP servers' tools in the agent's registry, beside the project's own: the fake server over
//! stdio, one that fails to start and one disabled. Calls come from a plain thread, as the
//! agent host's blocking threads make them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use krampsta_agent::{Access, Approval, Call, ToolOutput, ToolSpec, Toolbox};
use krampsta_mcp::{McpServers, ServerConfig, ServerState, ToolSet, Transport};
use tokio_util::sync::CancellationToken;

/// The project's own tools, as far as this test cares: one that answers "local".
struct Local;

impl Toolbox for Local {
    fn specs(&self) -> Arc<[ToolSpec]> {
        Arc::from([ToolSpec {
            name: "read_file".into(),
            description: "Reads.".into(),
            parameters: serde_json::json!({}),
        }])
    }

    fn run(&self, _: &str, _: &str, _: &Call<'_>) -> Result<ToolOutput, String> {
        Ok("local".to_owned().into())
    }
}

fn server(name: &str, args: &[&str], disabled: bool) -> ServerConfig {
    let transport = Transport::Stdio {
        command: env!("CARGO_BIN_EXE_krampsta-fake-mcp").into(),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        env: BTreeMap::new(),
        cwd: None,
    };
    ServerConfig { name: name.into(), transport, disabled }
}

fn started() -> (Arc<McpServers>, Arc<AtomicUsize>) {
    let changes = Arc::new(AtomicUsize::new(0));
    let counter = changes.clone();
    let configs = vec![server("fake", &[], false), server("broken", &["--crash"], false), server("off", &[], true)];
    let servers = McpServers::start(configs, Arc::new(move || _ = counter.fetch_add(1, Ordering::SeqCst))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while servers.statuses().iter().any(|(_, s)| matches!(s, ServerState::Connecting)) {
        assert!(Instant::now() < deadline, "the servers never finished connecting");
        std::thread::sleep(Duration::from_millis(10));
    }
    (servers, changes)
}

fn call(tools: &ToolSet, name: &str, arguments: &str) -> Result<ToolOutput, String> {
    let cancel = CancellationToken::new();
    tools.run(name, arguments, &Call { cancel: &cancel, approval: &Approval::Once, output: &|_| {} })
}

#[test]
fn a_servers_tools_join_the_projects_and_ask_before_running() {
    let (servers, changes) = started();
    assert_eq!(changes.load(Ordering::SeqCst), 2, "one wake per server that finished connecting");
    let statuses = servers.statuses();
    assert!(matches!(&statuses[0], (name, ServerState::Ready { tools, .. }) if name == "fake" && tools.len() == 5));
    assert!(
        matches!(&statuses[1], (name, ServerState::Failed(why)) if name == "broken" && why.contains("missing FAKE_TOKEN")),
        "{:?}",
        statuses[1]
    );
    assert!(matches!(&statuses[2], (_, ServerState::Disabled)));

    let tools = ToolSet::new(Some(Arc::new(Local)), servers);
    let specs = tools.specs();
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names[0], "read_file", "the project's own first");
    assert!(names.contains(&"mcp__fake__echo"), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("broken") || n.contains("off")));
    let echo = specs.iter().find(|s| s.name == "mcp__fake__echo").unwrap();
    assert!(echo.description.contains("from the MCP server `fake`"));
    assert_eq!(echo.parameters["required"][0], "text");

    let Access::Ask { what } = tools.check("mcp__fake__echo", r#"{"text":"hi"}"#) else {
        panic!("an MCP call asks first")
    };
    assert!(what.starts_with("echo on the MCP server `fake`\n") && what.contains("\"text\": \"hi\""), "{what}");
    assert_eq!(call(&tools, "mcp__fake__echo", r#"{"text": "hi"}"#).unwrap().text, "hi");
    assert_eq!(call(&tools, "mcp__fake__fail", "{}"), Err("it failed on purpose".into()));
    assert_eq!(call(&tools, "read_file", "{}").unwrap().text, "local");

    tools.remember("mcp__fake__echo", "{}").unwrap();
    assert_eq!(tools.check("mcp__fake__echo", "{}"), Access::Allow(Approval::Always));
    assert!(matches!(tools.check("mcp__fake__fail", "{}"), Access::Ask { .. }), "only the tool allowed");
}

#[test]
fn without_a_project_only_the_servers_tools_run() {
    let (servers, _) = started();
    let tools = ToolSet::new(None, servers);
    assert!(tools.specs().iter().all(|s| s.name.starts_with("mcp__")));
    assert_eq!(call(&tools, "mcp__fake__echo", r#"{"text": "no folder"}"#).unwrap().text, "no folder");
    assert!(call(&tools, "read_file", "{}").unwrap_err().contains("open one first"));
    assert!(call(&tools, "mcp__gone__x", "{}").unwrap_err().contains("isn't connected"));
}

#[test]
fn a_cancelled_call_stops() {
    let (servers, _) = started();
    let tools = ToolSet::new(None, servers);
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        stopper.cancel();
    });
    let started = Instant::now();
    let stopped =
        tools.run("mcp__fake__slow", "{}", &Call { cancel: &cancel, approval: &Approval::Once, output: &|_| {} });
    assert_eq!(stopped, Err("stopped".into()));
    assert!(started.elapsed() < Duration::from_secs(5));
}
