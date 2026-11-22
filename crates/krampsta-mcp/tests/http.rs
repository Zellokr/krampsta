//! The client over Streamable HTTP, against a small fake server on localhost that answers with
//! JSON (not SSE) and keeps a session id, as many servers do.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use krampsta_mcp::{McpClient, McpError, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

const SESSION: &str = "fake-session-1";

/// Serves the fake MCP server; `token`: the bearer token it requires, if any. Returns its URL.
async fn serve(token: Option<&'static str>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(connection(socket, token));
        }
    });
    url
}

/// One HTTP/1.1 connection, request after request until the client closes it.
async fn connection(mut socket: TcpStream, token: Option<&'static str>) {
    let mut buffer = Vec::new();
    loop {
        let Some(end) = read_until_headers(&mut socket, &mut buffer).await else { return };
        let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
        let header = |name: &str| {
            head.lines()
                .find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim()))
                .map(str::to_owned)
        };
        let length: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        while buffer.len() < end + 4 + length {
            let mut chunk = [0u8; 4096];
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            }
        }
        let body = buffer[end + 4..end + 4 + length].to_vec();
        buffer.drain(..end + 4 + length);

        let response = if !head.starts_with("POST ") {
            // No stream of server messages over GET, and nothing to end on DELETE.
            respond(405, "", "")
        } else if token.is_some_and(|t| header("authorization") != Some(format!("Bearer {t}"))) {
            respond(401, "www-authenticate: Bearer\r\n", "")
        } else {
            let message: serde_json::Value = serde_json::from_slice(&body).unwrap();
            match krampsta_mcp::fake::reply(&message) {
                Some(answer) => {
                    let session = if message["method"] == "initialize" {
                        format!("mcp-session-id: {SESSION}\r\n")
                    } else {
                        assert_eq!(header("mcp-session-id").as_deref(), Some(SESSION), "the session is kept");
                        String::new()
                    };
                    respond(200, &format!("content-type: application/json\r\n{session}"), &answer.to_string())
                }
                None => respond(202, "", ""),
            }
        };
        if socket.write_all(response.as_bytes()).await.is_err() {
            return;
        }
    }
}

async fn read_until_headers(socket: &mut TcpStream, buffer: &mut Vec<u8>) -> Option<usize> {
    loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            return Some(end);
        }
        let mut chunk = [0u8; 4096];
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    }
}

fn respond(status: u16, headers: &str, body: &str) -> String {
    format!("HTTP/1.1 {status} X\r\ncontent-length: {}\r\n{headers}\r\n{body}", body.len())
}

fn http(url: String, headers: &[(&str, &str)]) -> Transport {
    let headers = headers.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<BTreeMap<_, _>>();
    Transport::Http { url, headers }
}

#[tokio::test]
async fn it_lists_and_calls_tools_over_http() {
    let client = McpClient::connect(&http(serve(None).await, &[])).await.unwrap();
    assert_eq!(client.server_name(), Some("fake"));
    let tools = client.tools().await.unwrap();
    assert!(tools.iter().any(|t| t.name == "echo"));
    let reply = client.call("echo", r#"{"text": "over http"}"#, &CancellationToken::new()).await.unwrap();
    assert_eq!(reply.text, "over http");
    client.close().await.unwrap();
}

#[tokio::test]
async fn a_bearer_token_goes_in_the_authorization_header() {
    let url = serve(Some("sk-test")).await;
    let refused = McpClient::connect(&http(url.clone(), &[])).await.unwrap_err();
    assert!(matches!(refused, McpError::Connect { .. }), "{refused}");

    let client =
        McpClient::connect(&http(url, &[("Authorization", "Bearer sk-test"), ("X-Team", "core")])).await.unwrap();
    let reply = client.call("echo", r#"{"text": "let in"}"#, &CancellationToken::new()).await.unwrap();
    assert_eq!(reply.text, "let in");
}

#[tokio::test]
async fn a_bad_header_is_named_before_connecting() {
    let error = McpClient::connect(&http("http://127.0.0.1:9/mcp".into(), &[("Bad Header", "x")])).await.unwrap_err();
    assert!(matches!(error, McpError::Header(ref name) if name == "Bad Header"), "{error}");
}
