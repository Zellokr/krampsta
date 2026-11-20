//! The tools Krampsta's agent calls, confined to one project folder: `read_file`, `list_files`
//! and `search`. [`Workspace`] offers them to engines as a [`Toolbox`].
//!
//! Every call is synchronous file I/O, run by the host on a blocking thread; each checks the
//! run's cancellation as it goes and caps its output, so neither a huge repository nor a huge
//! file can stall a run or flood the model's context.

mod command;
mod edit;
mod list;
mod overlay;
mod policy;
mod project;
mod read;
mod search;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use krampsta_agent::{Access, Approval, Call, ToolOutput, ToolSpec, Toolbox};
use serde::de::DeserializeOwned;
use serde_json::json;

pub use policy::Policy;
pub use project::Project;

/// The project folder, the tools that work in it, the changes they have proposed, and the
/// rules for the commands they may run.
pub struct Workspace {
    project: Project,
    specs: Arc<[ToolSpec]>,
    pending: edit::Pending,
    policy: RwLock<Policy>,
    /// Krampsta's config folder: `permissions.json` and `commands.log`. None: rules can't be
    /// saved and nothing is logged.
    config: Option<PathBuf>,
}

impl Workspace {
    /// Tools over the folder at `root` (which must exist), with the user's rules from their
    /// config folder.
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Self::open_with(root, dirs::config_dir().map(|d| d.join("krampsta")))
    }

    /// Tools over `root` whose rules and log live in `config` (tests pass a temporary folder).
    pub fn open_with(root: impl AsRef<Path>, config: Option<PathBuf>) -> std::io::Result<Self> {
        let policy = match config.as_deref().map(|c| Policy::load(&c.join("permissions.json"))) {
            Some(Ok(policy)) => policy,
            Some(Err(e)) => {
                // With no rules every command asks: safe, if tedious, until the file is fixed.
                eprintln!("krampsta: {e}");
                Policy::default()
            }
            None => Policy::default(),
        };
        Ok(Workspace {
            project: Project::open(root)?,
            specs: specs(),
            pending: edit::Pending::default(),
            policy: RwLock::new(policy),
            config,
        })
    }

    pub fn root(&self) -> &Path {
        self.project.root()
    }

    fn run_command(&self, args: &command::Args, call: &Call<'_>) -> Result<ToolOutput, String> {
        let (output, outcome) = command::run(&self.project, args, call)?;
        self.log(&args.command, call.approval, &outcome);
        let text = format!("{output}\n\n(Command {}.)", outcome.describe());
        match outcome {
            command::Outcome::Exited(Some(0)) => Ok(text.into()),
            _ => Err(text),
        }
    }

    /// Appends one line to `commands.log`: when, where, what ran, who allowed it, how it ended.
    fn log(&self, command: &str, approval: &Approval, outcome: &command::Outcome) {
        let Some(config) = &self.config else { return };
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let entry = json!({
            "at": at,
            "folder": self.project.root().display().to_string(),
            "command": command,
            "allowed_by": command::approval_text(approval),
            "outcome": outcome.describe(),
        });
        let written = std::fs::create_dir_all(config).and_then(|()| {
            let mut log = std::fs::OpenOptions::new().create(true).append(true).open(config.join("commands.log"))?;
            writeln!(log, "{entry}")
        });
        if let Err(e) = written {
            eprintln!("krampsta: could not log the command: {e}");
        }
    }
}

impl Toolbox for Workspace {
    fn specs(&self) -> Arc<[ToolSpec]> {
        self.specs.clone()
    }

    fn run(&self, name: &str, arguments: &str, call: &Call<'_>) -> Result<ToolOutput, String> {
        let (project, pending, cancel) = (&self.project, &self.pending, call.cancel);
        match name {
            "read_file" => read::run(project, pending, parse(arguments)?, cancel).map(ToolOutput::from),
            "list_files" => list::run(project, pending, parse(arguments)?, cancel).map(ToolOutput::from),
            "search" => search::run(project, pending, parse(arguments)?, cancel).map(ToolOutput::from),
            "edit_file" => edit::run(project, pending, parse(arguments)?),
            "run_command" => self.run_command(&parse(arguments)?, call),
            other => Err(format!("there is no tool named `{other}`")),
        }
    }

    fn check(&self, name: &str, arguments: &str) -> Access {
        match (name, parse::<command::Args>(arguments)) {
            ("run_command", Ok(args)) => {
                self.policy.read().unwrap_or_else(PoisonError::into_inner).check(&args.command)
            }
            // Bad arguments fail in `run`, before anything runs.
            _ => Access::Allow(Approval::NotNeeded),
        }
    }

    fn remember(&self, name: &str, arguments: &str) -> Result<(), String> {
        let ("run_command", Ok(args)) = (name, parse::<command::Args>(arguments)) else { return Ok(()) };
        let rules = {
            let mut policy = self.policy.write().unwrap_or_else(PoisonError::into_inner);
            policy.allow(&args.command);
            policy.clone()
        };
        let config = self.config.as_ref().ok_or("there is no config folder to keep the rule in")?;
        rules.save(&config.join("permissions.json"))
    }

    fn write(&self, path: &str, expected: Option<&str>, content: &str) -> Result<(), String> {
        edit::write(&self.project, path, expected, content)
    }

    fn set_pending(&self, path: &str, content: Option<Arc<str>>) {
        self.pending.set(path, content);
    }
}

/// A call's arguments; providers send `""` for a call without any.
fn parse<T: DeserializeOwned>(arguments: &str) -> Result<T, String> {
    let arguments = if arguments.trim().is_empty() { "{}" } else { arguments };
    serde_json::from_str(arguments).map_err(|e| format!("the arguments are not valid: {e}"))
}

fn spec(name: &str, description: &str, parameters: serde_json::Value) -> ToolSpec {
    ToolSpec { name: name.into(), description: description.into(), parameters }
}

/// The tools' descriptions and argument schemas, as sent to the provider (built once).
fn specs() -> Arc<[ToolSpec]> {
    let run_command = spec(
        "run_command",
        &format!(
            "Run a {shell} command in the project folder: build, test, git and the like. The user \
             sees its output as it runs and must allow it unless one of their rules does. You get \
             stdout and stderr together (the first 8 KB and the last 24 KB) and the exit code. It \
             is stopped at its timeout, with everything it started, so never start servers or \
             watchers that don't exit. Prefer read_file, list_files and search for looking at files.",
            shell = command::SHELL
        ),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": format!("The {} command line.", command::SHELL)},
                "timeout_ms": {"type": "integer", "minimum": 1000, "maximum": 600000, "description": "Default: 120000 (2 minutes)."}
            },
            "required": ["command"]
        }),
    );
    Arc::from([
        spec(
            "list_files",
            "List the files and folders under a folder of the project, depth first in name order. \
             Files ignored by .gitignore (build output, dependencies) are left out. Folders end in `/`.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Folder relative to the project root. Default: the root."},
                    "depth": {"type": "integer", "minimum": 1, "description": "Levels to descend (1: only its own entries). Default: all."}
                }
            }),
        ),
        spec(
            "read_file",
            "Read numbered lines from the project. Pass path/offset/limit for one read, or ranges \
             for up to 32 reads in order, sharing one text budget. Batch results show actual lines, \
             disk/proposal source and next_offset when there is more.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the project root."},
                    "offset": {"type": "integer", "minimum": 1, "description": "First line to read (from 1). Default: 1."},
                    "limit": {"type": "integer", "minimum": 1, "description": "Lines to read. Default: 2000."},
                    "ranges": {
                        "type": "array", "minItems": 1, "maxItems": 32,
                        "description": "Use instead of top-level path, offset and limit.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": {"type": "string"},
                                "offset": {"type": "integer", "minimum": 1},
                                "limit": {"type": "integer", "minimum": 1}
                            },
                            "required": ["path"]
                        }
                    }
                }
            }),
        ),
        spec(
            "edit_file",
            "Propose a change to a file: replace old_string, which must appear exactly once (or use \
             replace_all), with new_string; with an empty old_string, create the file. Nothing is \
             written until the user accepts it; later reads and edits of the file see the proposal. \
             Read the file first and copy old_string from it exactly, indentation included.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the project root."},
                    "old_string": {"type": "string", "description": "The exact text to replace; empty to create the file."},
                    "new_string": {"type": "string", "description": "The text to put in its place."},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence. Default: false."}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        ),
        spec(
            "search",
            "Search the project's files for lines matching a regular expression (Rust regex syntax), \
             or plain text with `literal`. Files ignored by .gitignore and binary files are skipped. \
             Results are grouped by file with line numbers.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "The regex (or text, with literal) to find."},
                    "path": {"type": "string", "description": "Folder or file to search, relative to the root. Default: the whole project."},
                    "literal": {"type": "boolean", "description": "Match the pattern as plain text. Default: false."},
                    "glob": {"type": "string", "description": "Only files matching this glob, such as `*.rs` or `src/**/*.ts`."},
                    "case_sensitive": {"type": "boolean", "description": "Default: case-insensitive unless the pattern has a capital letter."},
                    "context": {"type": "integer", "minimum": 0, "maximum": 10, "description": "Lines before and after each match. Default: 0. Matches use ':', context uses '-'."},
                    "paths_only": {"type": "boolean", "description": "List matching paths only, once per file. Ignores context. Default: false."}
                },
                "required": ["pattern"]
            }),
        ),
        run_command,
    ])
}

#[cfg(test)]
mod tests {
    use krampsta_agent::CancellationToken;

    use super::*;

    /// A workspace over a temporary project, with its rules and log in a temporary config folder.
    fn workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, Workspace) {
        let (dir, config) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).unwrap();
        }
        let tools = Workspace::open_with(dir.path(), Some(config.path().to_owned())).unwrap();
        (dir, config, tools)
    }

    fn run(tools: &Workspace, name: &str, arguments: &str, cancel: &CancellationToken) -> Result<ToolOutput, String> {
        tools.run(name, arguments, &Call { cancel, approval: &Approval::NotNeeded, output: &|_| {} })
    }

    #[test]
    fn the_toolbox_dispatches_by_name_and_reports_bad_arguments() {
        let (_d, _c, tools) = workspace(&[("hello.txt", "hi\n")]);
        let cancel = CancellationToken::new();
        let specs = tools.specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["list_files", "read_file", "edit_file", "search", "run_command"]);
        assert_eq!(run(&tools, "list_files", "", &cancel).unwrap().text, "hello.txt\n");
        assert_eq!(run(&tools, "read_file", r#"{"path":"hello.txt"}"#, &cancel).unwrap().text, "     1\thi\n");
        assert!(run(&tools, "read_file", "{}", &cancel).unwrap_err().contains("missing field `path`"));
        assert!(run(&tools, "delete_everything", "{}", &cancel).unwrap_err().contains("no tool named"));
    }

    #[test]
    fn exploration_sees_proposals_without_writing_them() {
        let (dir, _config, tools) = workspace(&[("old.txt", "needle on disk\n"), (".gitignore", "ignored/\n")]);
        let cancel = CancellationToken::new();
        let tool = |name, args| run(&tools, name, args, &cancel).unwrap().text;
        tool("edit_file", r#"{"path":"old.txt","old_string":"on disk","new_string":"in proposal"}"#);
        tool("edit_file", r#"{"path":"new/nested/a.txt","old_string":"","new_string":"\nneedle new\n"}"#);
        tool("edit_file", r#"{"path":"ignored/b.txt","old_string":"","new_string":"needle ignored\n"}"#);

        assert_eq!(tool("list_files", r#"{"depth":1}"#), ".gitignore\nnew/\nold.txt\n");
        assert_eq!(tool("list_files", r#"{"path":"new"}"#), "new/nested/\nnew/nested/a.txt\n");
        assert_eq!(
            tool("search", r#"{"pattern":"needle"}"#),
            "new/nested/a.txt\n     2: needle new\nold.txt\n     1: needle in proposal\n"
        );
        assert_eq!(
            tool("search", r#"{"pattern":"needle","path":"new/nested/a.txt"}"#),
            "new/nested/a.txt\n     2: needle new\n"
        );
        assert_eq!(tool("search", r#"{"pattern":"needle","path":"new","glob":"*.rs"}"#), "No matches for `needle`.");
        assert_eq!(
            tool("search", r#"{"pattern":"needle","context":1}"#),
            "new/nested/a.txt\n     1- \n     2: needle new\nold.txt\n     1: needle in proposal\n"
        );
        assert_eq!(tool("search", r#"{"pattern":"needle","paths_only":true}"#), "new/nested/a.txt\nold.txt\n");
        let batch = tool(
            "read_file",
            r#"{"ranges":[{"path":"old.txt"},{"path":"new/nested/a.txt","offset":2},{"path":".gitignore"}]}"#,
        );
        assert_eq!(
            batch,
            concat!(
                "\"old.txt\" (proposal)\nlines: 1–1\n     1\tneedle in proposal\n\n",
                "\"new/nested/a.txt\" (proposal)\nlines: 2–2\n     2\tneedle new\n\n",
                "\".gitignore\" (disk)\nlines: 1–1\n     1\tignored/\n\n",
            )
        );
        assert_eq!(std::fs::read_to_string(dir.path().join("old.txt")).unwrap(), "needle on disk\n");
        assert!(!dir.path().join("new").exists());

        tools.set_pending("old.txt", None);
        tools.set_pending("new/nested/a.txt", None);
        assert_eq!(tool("search", r#"{"pattern":"needle"}"#), "old.txt\n     1: needle on disk\n");
        assert_eq!(tool("list_files", "{}"), ".gitignore\nold.txt\n");
    }

    #[test]
    fn a_cancelled_call_stops() {
        let (_d, _c, tools) = workspace(&[("a.txt", "a\n")]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(run(&tools, "list_files", "{}", &cancel), Err("stopped".to_owned()));
    }

    #[test]
    fn commands_ask_until_always_allowed_and_every_run_is_logged() {
        let (_d, config, tools) = workspace(&[]);
        let args = r#"{"command":"exit 0"}"#;
        assert_eq!(tools.check("run_command", args), Access::Ask { what: "exit 0".into() });
        assert_eq!(tools.check("read_file", r#"{"path":"x"}"#), Access::Allow(Approval::NotNeeded));

        tools.remember("run_command", args).unwrap();
        assert_eq!(tools.check("run_command", args), Access::Allow(Approval::Rule("exit 0".into())));
        let saved = Policy::load(&config.path().join("permissions.json")).unwrap();
        assert_eq!(saved.allow, ["exit 0"], "the rule outlives the app");

        let cancel = CancellationToken::new();
        let call = Call { cancel: &cancel, approval: &Approval::Rule("exit 0".into()), output: &|_| {} };
        assert!(tools.run("run_command", args, &call).unwrap().text.ends_with("(Command exit code 0.)"));
        let failed = tools.run("run_command", r#"{"command":"exit 2"}"#, &call).unwrap_err();
        assert!(failed.ends_with("(Command exit code 2.)"), "{failed}");

        let log = std::fs::read_to_string(config.path().join("commands.log")).unwrap();
        let lines: Vec<serde_json::Value> = log.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["command"], "exit 0");
        assert_eq!(lines[0]["allowed_by"], "rule `exit 0`");
        assert_eq!(lines[1]["outcome"], "exit code 2");
    }
}
