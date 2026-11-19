//! The tools Krampsta's agent calls, confined to one project folder: `read_file`, `list_files`
//! and `search`. [`Workspace`] offers them to engines as a [`Toolbox`].
//!
//! Every call is synchronous file I/O, run by the host on a blocking thread; each checks the
//! run's cancellation as it goes and caps its output, so neither a huge repository nor a huge
//! file can stall a run or flood the model's context.

mod list;
mod project;
mod read;
mod search;

use std::path::Path;
use std::sync::Arc;

use krampsta_agent::{CancellationToken, ToolSpec, Toolbox};
use serde::de::DeserializeOwned;
use serde_json::json;

pub use project::Project;

/// The project folder and the tools that work in it.
pub struct Workspace {
    project: Project,
    specs: Arc<[ToolSpec]>,
}

impl Workspace {
    /// Tools over the folder at `root` (which must exist).
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Workspace { project: Project::open(root)?, specs: specs() })
    }

    pub fn root(&self) -> &Path {
        self.project.root()
    }
}

impl Toolbox for Workspace {
    fn specs(&self) -> Arc<[ToolSpec]> {
        self.specs.clone()
    }

    fn run(&self, name: &str, arguments: &str, cancel: &CancellationToken) -> Result<String, String> {
        let project = &self.project;
        match name {
            "read_file" => read::run(project, parse(arguments)?, cancel),
            "list_files" => list::run(project, parse(arguments)?, cancel),
            "search" => search::run(project, parse(arguments)?, cancel),
            other => Err(format!("there is no tool named `{other}`")),
        }
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
            "Read a text file of the project. Lines come numbered; long files come in parts and say \
             which offset reads on.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the project root."},
                    "offset": {"type": "integer", "minimum": 1, "description": "First line to read (from 1). Default: 1."},
                    "limit": {"type": "integer", "minimum": 1, "description": "Lines to read. Default: 2000."}
                },
                "required": ["path"]
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
                    "case_sensitive": {"type": "boolean", "description": "Default: case-insensitive unless the pattern has a capital letter."}
                },
                "required": ["pattern"]
            }),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toolbox_dispatches_by_name_and_reports_bad_arguments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hi\n").unwrap();
        let tools = Workspace::open(dir.path()).unwrap();
        let cancel = CancellationToken::new();
        let specs = tools.specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["list_files", "read_file", "search"]);
        assert_eq!(tools.run("list_files", "", &cancel).unwrap(), "hello.txt\n");
        assert_eq!(tools.run("read_file", r#"{"path":"hello.txt"}"#, &cancel).unwrap(), "     1\thi\n");
        assert!(tools.run("read_file", "{}", &cancel).unwrap_err().contains("missing field `path`"));
        assert!(tools.run("delete_everything", "{}", &cancel).unwrap_err().contains("no tool named"));
    }

    #[test]
    fn a_cancelled_call_stops() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        let tools = Workspace::open(dir.path()).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(tools.run("list_files", "{}", &cancel), Err("stopped".into()));
    }
}
