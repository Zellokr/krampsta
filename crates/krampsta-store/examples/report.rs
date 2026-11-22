//! What the latest saved sessions cost, as a Markdown table: tool calls (by tool), failed calls,
//! how much text the tools sent back to the model, and the tokens each session used. For
//! comparing runs of the same task (with and without an MCP server, say).
//!
//! ```text
//! cargo run -p krampsta-store --example report                  # the 6 newest sessions
//! cargo run -p krampsta-store --example report -- 10 --title "callers of"
//! cargo run -p krampsta-store --example report -- --dir <sessions folder>
//! ```
//!
//! It reads only titles, counts and tool names: no message, argument or output text is printed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use krampsta_core::{MessageRole, Session};

struct Options {
    count: usize,
    title: Option<String>,
    dir: Option<PathBuf>,
}

fn options() -> Result<Options, String> {
    let mut options = Options { count: 6, title: None, dir: None };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--title" => options.title = Some(args.next().ok_or("--title needs a text")?.to_lowercase()),
            "--dir" => options.dir = Some(args.next().ok_or("--dir needs a folder")?.into()),
            n => options.count = n.parse().map_err(|_| format!("`{n}` isn't a number of sessions"))?,
        }
    }
    Ok(options)
}

fn main() {
    let options = match options() {
        Ok(options) => options,
        Err(why) => {
            eprintln!("report: {why}");
            std::process::exit(2);
        }
    };
    let Some(dir) = options.dir.or_else(krampsta_store::default_dir) else {
        eprintln!("report: no data folder on this system; pass --dir");
        std::process::exit(2);
    };
    let loaded = krampsta_store::load(&dir);
    for failure in &loaded.failures {
        eprintln!("report: {failure}");
    }
    let wanted = |s: &&Session| options.title.as_ref().is_none_or(|t| s.title.to_lowercase().contains(t));
    let sessions: Vec<&Session> = loaded.sessions.iter().filter(wanted).take(options.count).collect();
    if sessions.is_empty() {
        eprintln!("report: no saved sessions match in {}", dir.display());
        return;
    }
    println!("| Session | Model | Tool calls | By tool | Failed | Tool output | Tokens in | Tokens out |");
    println!("|---|---|---:|---|---:|---:|---:|---:|");
    for session in sessions {
        println!("{}", row(session));
    }
}

/// One session's line.
fn row(session: &Session) -> String {
    let calls: Vec<_> =
        session.messages.iter().filter(|m| m.role == MessageRole::Assistant).flat_map(|m| &m.tool_calls).collect();
    let results: Vec<_> = session.messages.iter().flat_map(|m| &m.tool_results).collect();
    let mut by_tool = BTreeMap::<&str, usize>::new();
    for call in &calls {
        *by_tool.entry(call.name.as_str()).or_default() += 1;
    }
    let mut by_tool: Vec<_> = by_tool.into_iter().collect();
    by_tool.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let by_tool = by_tool.iter().map(|(name, n)| format!("{name} ×{n}")).collect::<Vec<_>>().join(", ");
    let failed = results.iter().filter(|r| r.is_error).count();
    let output: usize = results.iter().map(|r| r.output.len()).sum();
    let model = session.by_model.iter().map(|m| m.model.as_str()).collect::<Vec<_>>().join(", ");
    // Pipes would break the table.
    let title = session.title.replace('|', "/");
    format!(
        "| {title} | {model} | {} | {by_tool} | {failed} | {} KB | {} | {} |",
        calls.len(),
        output.div_ceil(1024),
        session.usage.input,
        session.usage.output
    )
}
