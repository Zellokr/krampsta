//! The client against a real process: the fake server in `src/bin/fake_mcp.rs`, over stdio.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use krampsta_mcp::{MAX_REPLY, McpClient, McpError, Transport};
use tokio_util::sync::CancellationToken;

fn fake(args: &[&str]) -> Transport {
    Transport::Stdio {
        command: env!("CARGO_BIN_EXE_krampsta-fake-mcp").into(),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        env: BTreeMap::new(),
        cwd: None,
    }
}

#[tokio::test]
async fn it_lists_a_servers_tools_and_calls_them() {
    let client = McpClient::connect(&fake(&[])).await.unwrap();
    assert_eq!(client.server_name(), Some("fake"));

    let tools = client.tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["echo", "fail", "big", "mixed", "slow"]);
    assert_eq!(tools[0].description, "Says the text back.");
    assert_eq!(tools[0].input_schema["required"][0], "text");

    let cancel = CancellationToken::new();
    let echo = client.call("echo", r#"{"text": "hola, ñandú"}"#, &cancel).await.unwrap();
    assert_eq!((echo.text.as_str(), echo.is_error), ("hola, ñandú", false));

    let failed = client.call("fail", "", &cancel).await.unwrap();
    assert!(failed.is_error, "a tool's own failure is a reply, not a client error");
    assert_eq!(failed.text, "it failed on purpose");

    let big = client.call("big", "{}", &cancel).await.unwrap();
    assert!(big.text.len() < MAX_REPLY + 64 && big.text.ends_with("bytes left out)"), "capped");

    let mixed = client.call("mixed", "{}", &cancel).await.unwrap();
    assert_eq!(mixed.text, "a chart:\n[image: image/png]\n[link: file:///report.md]");

    let unknown = client.call("nope", "{}", &cancel).await.unwrap_err();
    assert!(matches!(unknown, McpError::Service(ref e) if e.contains("unknown tool")), "{unknown}");
    let bad = client.call("echo", "[1, 2]", &cancel).await.unwrap_err();
    assert!(matches!(bad, McpError::Arguments(_)), "{bad}");

    client.close().await.unwrap();
}

#[tokio::test]
async fn a_call_stops_waiting_when_cancelled() {
    let client = McpClient::connect(&fake(&[])).await.unwrap();
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        stopper.cancel();
    });
    let started = Instant::now();
    let stopped = client.call("slow", "{}", &cancel).await.unwrap_err();
    assert!(matches!(stopped, McpError::Cancelled), "{stopped}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn a_server_that_dies_at_start_says_why() {
    let error = McpClient::connect(&fake(&["--crash"])).await.unwrap_err();
    let message = error.to_string();
    assert!(matches!(error, McpError::Connect { .. }), "{message}");
    assert!(message.contains("missing FAKE_TOKEN"), "its stderr is in the error: {message}");
}

#[tokio::test]
async fn a_command_that_doesnt_exist_is_named() {
    let missing =
        Transport::Stdio { command: "krampsta-no-such-server".into(), args: vec![], env: BTreeMap::new(), cwd: None };
    let error = McpClient::connect(&missing).await.unwrap_err();
    assert!(matches!(error, McpError::Start { .. }), "{error}");
    assert!(error.to_string().contains("krampsta-no-such-server"));
}
