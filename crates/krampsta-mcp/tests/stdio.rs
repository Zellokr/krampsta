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
async fn a_server_sees_only_the_basics_and_its_own_variables() {
    let mut server = fake(&[]);
    if let Transport::Stdio { env, .. } = &mut server {
        env.insert("FAKE_TOKEN".into(), "configured".into());
    }
    let client = McpClient::connect(&server).await.unwrap();
    let names = client.call("env", "{}", &CancellationToken::new()).await.unwrap().text;
    let names: Vec<String> = names.lines().map(str::to_uppercase).collect();
    assert!(names.contains(&"FAKE_TOKEN".to_owned()), "its configured variables: {names:?}");
    assert!(names.contains(&"PATH".to_owned()), "what programs need to run: {names:?}");
    // Cargo sets these for the test process; the server must not inherit them (as it must not
    // inherit an OPENAI_API_KEY from Krampsta's).
    assert!(!names.iter().any(|n| n.starts_with("CARGO_")), "nothing else of ours: {names:?}");
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

/// Against the reference server, through npx (needs Node and the network; run by hand with
/// `cargo test -p krampsta-mcp --test stdio -- --ignored`).
#[tokio::test]
#[ignore = "downloads and runs @modelcontextprotocol/server-everything with npx"]
async fn a_real_server_over_npx() {
    let everything = Transport::Stdio {
        command: "npx".into(),
        args: vec!["-y".into(), "@modelcontextprotocol/server-everything".into()],
        env: BTreeMap::new(),
        cwd: None,
    };
    let client = McpClient::connect(&everything).await.unwrap();
    eprintln!("server: {:?}", client.server_name());
    let tools = client.tools().await.unwrap();
    eprintln!("tools: {:?}", tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>());
    let cancel = CancellationToken::new();
    let echo = client.call("echo", r#"{"message": "hola desde Krampsta"}"#, &cancel).await.unwrap();
    eprintln!("echo: {echo:?}");
    assert!(echo.text.contains("hola desde Krampsta") && !echo.is_error);
    // The test process carries Cargo's variables; the server must see none of them.
    let seen = client.call("get-env", "{}", &cancel).await.unwrap().text;
    assert!(seen.contains("PATH") && !seen.contains("CARGO_"), "it inherits only the basics");
    for tool in tools.iter().filter(|t| t.name != "echo") {
        // Whatever each answers with arguments it may not accept: never a hang or a panic.
        let started = Instant::now();
        let reply = tokio::time::timeout(Duration::from_secs(20), client.call(&tool.name, "{}", &cancel)).await;
        eprintln!(
            "{} ({:?}): {:?}",
            tool.name,
            started.elapsed(),
            reply.map(|r| r.map(|r| r.text.chars().take(80).collect::<String>()))
        );
    }
    client.close().await.unwrap();
}
