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
mod snapshot;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use krampsta_agent::{Access, Approval, Call, OpenedProject, ProjectOpener, ToolOutput, ToolSpec, Toolbox};
use serde::de::DeserializeOwned;
use serde_json::json;

pub use policy::Policy;
pub use project::Project;

/// Opens projects as [`Workspace`]s, with the user's rules from their config folder.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkspaceOpener;

impl ProjectOpener for WorkspaceOpener {
    fn open(&self, root: &Path) -> Result<OpenedProject, String> {
        Workspace::open_project(root)
    }
}

/// Why `root` can't be the project, in words a person can act on.
fn folder_error(root: &Path, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("{} doesn't exist", root.display()),
        std::io::ErrorKind::NotADirectory => format!("{} is not a folder", root.display()),
        _ => format!("{} can't be opened as the project: {e}", root.display()),
    }
}

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
    /// Where Cargo builds when a command checks the pending edits (`pending: true`), kept
    /// between checks. None: in the throwaway copy.
    check_target: Option<PathBuf>,
}

impl Workspace {
    /// Tools over the folder at `root` (which must exist), with the user's rules from their
    /// config folder, and checks of pending edits building in the user's cache folder.
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let workspace = Self::open_with(root, dirs::config_dir().map(|d| d.join("krampsta")))?;
        Ok(match dirs::cache_dir() {
            Some(cache) => workspace.with_check_target(cache.join("krampsta").join("check-target")),
            None => workspace,
        })
    }

    /// Checks of pending edits build in `target`, which outlives them (tests pass a temporary
    /// folder).
    pub fn with_check_target(mut self, target: PathBuf) -> Self {
        self.check_target = Some(target);
        self
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
            check_target: None,
        })
    }

    pub fn root(&self) -> &Path {
        self.project.root()
    }

    /// Opens `root` as the app opens a project, the [`OpenedProject`] the UI switches to.
    pub fn open_project(root: &Path) -> Result<OpenedProject, String> {
        Self::open(root).map(Self::into_project).map_err(|e| folder_error(root, &e))
    }

    /// This workspace as the project the UI shows.
    pub fn into_project(self) -> OpenedProject {
        OpenedProject { root: self.root().to_owned(), tools: Arc::new(self) }
    }

    fn run_command(&self, args: &command::Args, call: &Call<'_>) -> Result<ToolOutput, String> {
        let snapshot = if args.pending {
            Some(snapshot::Snapshot::create(&self.project, &self.pending, call.cancel)?)
        } else {
            None
        };
        let project = snapshot.as_ref().map_or(&self.project, |s| &s.project);
        // The copy is thrown away, but its build isn't: Cargo keeps it in a folder of its own
        // (outside the project), so the next check doesn't rebuild the dependencies. Without one, it
        // builds inside the copy, from scratch.
        let target =
            snapshot.as_ref().map(|s| self.check_target.clone().unwrap_or_else(|| s.project.root().join("target")));
        let context = snapshot.as_ref().map_or_else(String::new, |s| {
            format!("Running on a temporary copy with {} pending file(s), not the project on disk.\n", s.edits)
        });
        if !context.is_empty() {
            (call.output)(&context);
        }
        let (output, outcome) = command::run(project, args, target.as_deref(), call)?;
        self.log(&args.command, args.pending, call.approval, &outcome);
        let text = format!("{context}{output}\n\n(Command {}.)", outcome.describe());
        match outcome {
            command::Outcome::Exited(Some(0)) => Ok(text.into()),
            _ => Err(text),
        }
    }

    /// Appends one line to `commands.log`: when, where, what ran, who allowed it, how it ended.
    fn log(&self, command: &str, pending: bool, approval: &Approval, outcome: &command::Outcome) {
        let Some(config) = &self.config else { return };
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let entry = json!({
            "at": at,
            "folder": self.project.root().display().to_string(),
            "command": command,
            "pending": pending,
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
                match self.policy.read().unwrap_or_else(PoisonError::into_inner).check(&args.command) {
                    Access::Ask { what } if args.pending => Access::Ask { what: format!("{what} (pending edits)") },
                    access => access,
                }
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
             watchers that don't exit. Prefer read_file, list_files and search for looking at files, \
             and never change files with a command (scripts, sed, Set-Content, `>`): use edit_file, \
             which shows the user each change to review. Set pending to true to check proposed \
             edits in a disposable copy instead of running against the project on disk.",
            shell = command::SHELL
        ),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": format!("The {} command line.", command::SHELL)},
                "pending": {"type": "boolean", "description": "Default: false. Run against a temporary copy containing the pending edits. The copy excludes ignored files and the root target directory; this is not a sandbox."},
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
    use krampsta_agent::{CancellationToken, project_name};

    use super::*;

    /// A workspace over a temporary project, with its rules and log in a temporary config folder.
    fn workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, Workspace) {
        let (dir, config) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for (name, body) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        let tools = Workspace::open_with(dir.path(), Some(config.path().to_owned())).unwrap();
        (dir, config, tools)
    }

    fn run(tools: &Workspace, name: &str, arguments: &str, cancel: &CancellationToken) -> Result<ToolOutput, String> {
        tools.run(name, arguments, &Call { cancel, approval: &Approval::NotNeeded, output: &|_| {} })
    }

    #[test]
    fn a_workspace_becomes_the_project_named_after_its_resolved_folder() {
        let (dir, _c, tools) = workspace(&[("hello.txt", "hi\n")]);
        let project = tools.into_project();
        assert_eq!(project.root, dir.path().canonicalize().unwrap());
        assert_eq!(project.name(), project_name(&dir.path().canonicalize().unwrap()));
        assert!(project.tools.specs().iter().any(|s| s.name == "read_file"));
    }

    #[test]
    fn folders_that_cant_be_projects_say_why() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "").unwrap();
        let missing = dir.path().join("gone");
        let not_found = Project::open(&missing).unwrap_err();
        assert_eq!(folder_error(&missing, &not_found), format!("{} doesn't exist", missing.display()));
        let not_dir = Project::open(&file).unwrap_err();
        assert_eq!(folder_error(&file, &not_dir), format!("{} is not a folder", file.display()));
    }

    #[test]
    fn the_toolbox_dispatches_by_name_and_reports_bad_arguments() {
        let (_d, _c, tools) = workspace(&[("hello.txt", "hi\n")]);
        let cancel = CancellationToken::new();
        let specs = tools.specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["list_files", "read_file", "edit_file", "search", "run_command"]);
        // Changes go through edit_file, which the user reviews, never through a command.
        let run_command = &specs[4].description;
        assert!(run_command.contains("never change files with a command") && run_command.contains("use edit_file"));
        assert!(!run_command.contains('\n'), "one paragraph, no stray line breaks");
        assert_eq!(run(&tools, "list_files", "", &cancel).unwrap().text, "hello.txt\n");
        assert_eq!(run(&tools, "read_file", r#"{"path":"hello.txt"}"#, &cancel).unwrap().text, "     1\thi\n");
        assert!(run(&tools, "read_file", "{}", &cancel).unwrap_err().contains("missing field `path`"));
        assert!(run(&tools, "delete_everything", "{}", &cancel).unwrap_err().contains("no tool named"));
    }

    #[test]
    fn proposals_are_hidden_as_git_would_hide_them_if_they_were_on_disk() {
        let (dir, _config, tools) = workspace(&[(".gitignore", "target/\n!target/keep.rs\n*.log\n")]);
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/.gitignore"), "local.txt\n").unwrap();
        std::fs::create_dir_all(dir.path().join(".git/info")).unwrap();
        std::fs::write(dir.path().join(".git/info/exclude"), "secret.txt\n").unwrap();
        let cancel = CancellationToken::new();
        let tool = |name, args: &str| run(&tools, name, args, &cancel).unwrap().text;
        for path in ["target/keep.rs", "debug.log", "sub/local.txt", "secret.txt", "sub/ok.txt"] {
            tool("edit_file", &format!(r#"{{"path":"{path}","old_string":"","new_string":"needle\n"}}"#));
        }

        // `!target/keep.rs` can't bring back a file inside an excluded folder, as in git; a file
        // pattern, a subfolder's own rules and .git/info/exclude all apply to proposals too.
        assert_eq!(tool("list_files", "{}"), ".gitignore\nsub/\nsub/.gitignore\nsub/ok.txt\n");
        assert_eq!(tool("search", r#"{"pattern":"needle"}"#), "sub/ok.txt\n     1: needle\n");
        // A proposal still reads by its path, ignored or not: the agent wrote it.
        assert_eq!(tool("read_file", r#"{"path":"target/keep.rs"}"#), "     1\tneedle\n");
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
    fn a_pending_check_runs_in_a_copy_with_the_edits_and_builds_outside_it() {
        let (dir, config, tools) = workspace(&[("src/lib.rs", "pub fn original() {}\n"), ("README.md", "hi\n")]);
        let target = tempfile::tempdir().unwrap();
        let tools = tools.with_check_target(target.path().to_owned());
        tools.set_pending("src/lib.rs", Some("pub fn proposed() {}\n".into()));
        let cancel = CancellationToken::new();
        let call = Call { cancel: &cancel, approval: &Approval::Once, output: &|_| {} };
        // What a check sees: the pending source, the untouched files, and where Cargo builds.
        let command = if cfg!(windows) {
            "Get-Content src/lib.rs, README.md; $env:CARGO_TARGET_DIR"
        } else {
            "cat src/lib.rs README.md; echo \"$CARGO_TARGET_DIR\""
        };
        let proposed = json!({ "command": command, "pending": true }).to_string();
        assert_eq!(tools.check("run_command", &proposed), Access::Ask { what: format!("{command} (pending edits)") });
        let output = tools.run("run_command", &proposed, &call).unwrap().text;
        assert!(output.contains("temporary copy with 1 pending file"), "{output}");
        assert!(output.contains("pub fn proposed()") && output.contains("hi"), "{output}");
        assert!(output.contains(&target.path().display().to_string()), "Cargo builds outside the copy: {output}");
        assert_eq!(std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(), "pub fn original() {}\n");
        let log = std::fs::read_to_string(config.path().join("commands.log")).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(log.trim()).unwrap()["pending"], true);

        // Without pending edits there is nothing to check.
        tools.set_pending("src/lib.rs", None);
        let refused = tools.run("run_command", &proposed, &call).unwrap_err();
        assert!(refused.contains("no pending edits"), "{refused}");
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
