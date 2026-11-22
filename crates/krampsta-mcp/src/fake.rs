//! A fake MCP server's answers, for tests: the stdio binary (`src/bin/fake_mcp.rs`) and the
//! HTTP server in `tests/http.rs` both answer with [`reply`]. It speaks an older protocol
//! version, so the client's negotiation down is tested too.

use serde_json::{Value, json};

/// The version the fake server answers `initialize` with.
pub const PROTOCOL: &str = "2025-06-18";

/// The answer to one JSON-RPC message; `None` for a notification.
pub fn reply(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message["method"].as_str().unwrap_or_default();
    let result = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fake", "version": "1.0.0"},
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": [
            {
                "name": "echo",
                "description": "Says the text back.",
                "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]},
            },
            {"name": "fail", "description": "Always fails.", "inputSchema": {"type": "object"}},
            {"name": "big", "description": "Answers with a lot of text.", "inputSchema": {"type": "object"}},
            {"name": "mixed", "description": "Text, an image and a link.", "inputSchema": {"type": "object"}},
            {"name": "slow", "description": "Never answers (stdio only).", "inputSchema": {"type": "object"}},
        ]}),
        "tools/call" => {
            let arguments = &message["params"]["arguments"];
            match message["params"]["name"].as_str().unwrap_or_default() {
                "echo" => text(arguments["text"].as_str().unwrap_or_default(), false),
                "fail" => text("it failed on purpose", true),
                "big" => text(&"x".repeat(200_000), false),
                "mixed" => json!({"content": [
                    {"type": "text", "text": "a chart:"},
                    {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"},
                    {"type": "resource_link", "uri": "file:///report.md", "name": "report"},
                ]}),
                name => return Some(error(id, -32602, &format!("unknown tool {name}"))),
            }
        }
        _ => return Some(error(id, -32601, &format!("unknown method {method}"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn text(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
