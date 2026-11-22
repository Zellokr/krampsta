//! Changes made from Settings: a new server checked as the form gives it, and the edits to
//! `mcp.json` that keep it in step. Each edit touches only its server's entry and keeps the rest
//! of the file as it was, keys Krampsta doesn't use included (a file copied from Claude Desktop
//! holds more than servers).

use std::collections::BTreeMap;
use std::path::Path;

use krampsta_agent::{NewServer, ServerKind};
use serde_json::{Map, Value};

use crate::Transport;
use crate::config::{ServerConfig, valid_name};

/// One change to the file.
#[derive(Debug)]
pub(crate) enum Edit {
    Add(ServerConfig),
    Remove(String),
    Enabled(String, bool),
}

/// The server the form describes, or why it can't be one. `taken`: whether a name is in use.
pub(crate) fn server_config(new: &NewServer, taken: impl Fn(&str) -> bool) -> Result<ServerConfig, String> {
    let name = new.name.trim();
    if name.is_empty() {
        return Err("Give the server a name.".into());
    }
    if !valid_name(name) {
        return Err("A server's name has only letters, digits, `-` and `_`.".into());
    }
    if taken(name) {
        return Err(format!("There is already a server named `{name}`."));
    }
    let target = new.target.trim();
    let transport = match new.kind {
        ServerKind::Command => {
            let mut words = split_command(target)?.into_iter();
            let command = words.next().ok_or("Write the command that starts the server.")?;
            let env = pairs(&new.extra, '=', "KEY=value")?;
            Transport::Stdio { command, args: words.collect(), env, cwd: None }
        }
        ServerKind::Url => {
            if !(target.starts_with("http://") || target.starts_with("https://")) {
                return Err("The URL starts with http:// or https://.".into());
            }
            let headers = pairs(&new.extra, ':', "Name: value")?;
            Transport::Http { url: target.to_owned(), headers }
        }
    };
    Ok(ServerConfig { name: name.to_owned(), transport, disabled: false })
}

/// A command line in words: split at spaces, with "double" or 'single' quotes keeping spaces in
/// a word. Backslashes are plain characters (Windows paths).
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote = None;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err("A quote in the command isn't closed.".into());
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// `KEY=value` (or `Name: value`) lines; blank lines are skipped.
fn pairs(text: &str, separator: char, shape: &str) -> Result<BTreeMap<String, String>, String> {
    let mut pairs = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (key, value) = line.split_once(separator).ok_or_else(|| format!("Write each line as {shape}."))?;
        let key = key.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            return Err(format!("Write each line as {shape}, with no spaces in the name."));
        }
        pairs.insert(key.to_owned(), value.trim().to_owned());
    }
    Ok(pairs)
}

/// Applies `edit` to `file`, creating it if it doesn't exist, through a temporary file renamed
/// over it. Blocking.
pub(crate) fn apply(file: &Path, edit: &Edit) -> Result<(), String> {
    let shown = file.display();
    let mut root = match std::fs::read_to_string(file) {
        Ok(text) => serde_json::from_str::<Value>(&text).map_err(|e| format!("{shown} isn't valid: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(e) => return Err(format!("{shown} can't be read: {e}")),
    };
    let servers = root
        .as_object_mut()
        .ok_or_else(|| format!("{shown} isn't a JSON object"))?
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| format!("\"mcpServers\" in {shown} isn't an object"))?;
    match edit {
        Edit::Add(config) => {
            let entry = serde_json::to_value(&config.transport).map_err(|e| e.to_string())?;
            servers.insert(config.name.clone(), entry);
        }
        Edit::Remove(name) => {
            servers.remove(name);
        }
        Edit::Enabled(name, enabled) => {
            let entry = servers
                .get_mut(name)
                .and_then(Value::as_object_mut)
                .ok_or_else(|| format!("`{name}` isn't in {shown} any more"))?;
            if *enabled {
                entry.remove("disabled");
            } else {
                entry.insert("disabled".into(), Value::Bool(true));
            }
        }
    }
    let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())? + "\n";
    let temp = file.with_extension("json.tmp");
    file.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&temp, text))
        .and_then(|()| std::fs::rename(&temp, file))
        .map_err(|e| format!("{shown} can't be saved: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load;

    fn new(name: &str, kind: ServerKind, target: &str, extra: &str) -> NewServer {
        NewServer { name: name.into(), kind, target: target.into(), extra: extra.into() }
    }

    #[test]
    fn a_command_line_splits_into_words_keeping_quoted_spaces() {
        assert_eq!(split_command("npx -y  server").unwrap(), ["npx", "-y", "server"]);
        assert_eq!(
            split_command(r#""C:\Program Files\node\node.exe" 'my server.js' --name="a b""#).unwrap(),
            [r"C:\Program Files\node\node.exe", "my server.js", "--name=a b"]
        );
        assert_eq!(split_command(r#"run "" end"#).unwrap(), ["run", "", "end"], "an empty quoted word stays");
        assert!(split_command("npx \"unclosed").is_err());
    }

    #[test]
    fn the_form_becomes_a_server_or_says_what_is_wrong() {
        let free = |_: &str| false;
        let command =
            server_config(&new(" github ", ServerKind::Command, "npx -y server-github", "GITHUB_TOKEN = t\n\n"), free)
                .unwrap();
        assert_eq!(command.name, "github");
        assert!(matches!(&command.transport, Transport::Stdio { command, args, env, .. }
            if command == "npx" && args == &["-y", "server-github"] && env["GITHUB_TOKEN"] == "t"));
        let url =
            server_config(&new("docs", ServerKind::Url, "https://x.dev/mcp", "Authorization: Bearer t"), free).unwrap();
        assert!(matches!(&url.transport, Transport::Http { headers, .. } if headers["Authorization"] == "Bearer t"));

        let cases = [
            (new("", ServerKind::Command, "x", ""), "Give the server a name"),
            (new("my server", ServerKind::Command, "x", ""), "letters, digits"),
            (new("a", ServerKind::Command, "  ", ""), "Write the command"),
            (new("a", ServerKind::Command, "x", "NO_EQUALS"), "KEY=value"),
            (new("a", ServerKind::Url, "ftp://x", ""), "http:// or https://"),
            (new("a", ServerKind::Url, "https://x", "Bad Name: v"), "no spaces"),
        ];
        for (form, expected) in cases {
            let error = server_config(&form, free).unwrap_err();
            assert!(error.contains(expected), "{form:?}: {error}");
        }
        let taken = server_config(&new("github", ServerKind::Command, "x", ""), |n| n == "github").unwrap_err();
        assert!(taken.contains("already a server named `github`"));
    }

    #[test]
    fn edits_touch_only_their_server_and_keep_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mcp.json");
        std::fs::write(
            &file,
            r#"{"globalShortcut": "Ctrl+Space",
                "mcpServers": {"old": {"command": "old", "autoApprove": ["x"]}}}"#,
        )
        .unwrap();
        let docs = server_config(&new("docs", ServerKind::Url, "https://x.dev/mcp", ""), |_| false).unwrap();
        apply(&file, &Edit::Add(docs)).unwrap();
        apply(&file, &Edit::Enabled("old".into(), false)).unwrap();

        let servers = load(&file).unwrap();
        assert_eq!(servers.len(), 2);
        assert!(servers.iter().any(|s| s.name == "docs" && !s.disabled));
        assert!(servers.iter().any(|s| s.name == "old" && s.disabled));
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(raw["globalShortcut"], "Ctrl+Space", "keys Krampsta doesn't use stay");
        assert_eq!(raw["mcpServers"]["old"]["autoApprove"][0], "x", "and so do a server's own");

        apply(&file, &Edit::Enabled("old".into(), true)).unwrap();
        apply(&file, &Edit::Remove("docs".into())).unwrap();
        let servers = load(&file).unwrap();
        assert_eq!(servers.len(), 1);
        assert!(!servers[0].disabled);
        assert!(!dir.path().join("mcp.json.tmp").exists());
    }

    #[test]
    fn a_missing_file_is_created_and_a_broken_one_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("krampsta/mcp.json");
        let docs = server_config(&new("docs", ServerKind::Url, "https://x.dev/mcp", ""), |_| false).unwrap();
        apply(&file, &Edit::Add(docs)).unwrap();
        assert_eq!(load(&file).unwrap().len(), 1);

        std::fs::write(&file, "{ broken").unwrap();
        assert!(apply(&file, &Edit::Remove("docs".into())).unwrap_err().contains("isn't valid"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ broken");
    }
}
