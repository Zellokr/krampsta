//! Opening a project folder: the tools the agent gets over it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::Toolbox;

/// A folder opened as the project: where it is and the tools over it.
#[derive(Clone)]
pub struct OpenedProject {
    /// The folder, as the tools resolved it (absolute).
    pub root: PathBuf,
    pub tools: Arc<dyn Toolbox>,
}

impl std::fmt::Debug for OpenedProject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenedProject").field("root", &self.root).finish_non_exhaustive()
    }
}

impl OpenedProject {
    /// The folder's name, as a person calls the project.
    pub fn name(&self) -> String {
        project_name(&self.root)
    }
}

/// The last part of `root` (the whole path for a drive or `/`).
pub fn project_name(root: &Path) -> String {
    root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Builds the tools over a folder (`krampsta-tools` does it for the app).
pub trait ProjectOpener: Send + Sync {
    /// Opens `root` as the project. May block (it checks the folder and reads the user's
    /// rules): [`AgentHost::open_project`](crate::AgentHost::open_project) calls it off the UI
    /// thread. `Err` is a reason a person can read.
    fn open(&self, root: &Path) -> Result<OpenedProject, String>;
}

/// The answer to [`AgentHost::open_project`](crate::AgentHost::open_project).
#[derive(Debug)]
pub struct ProjectOpened {
    /// The folder that was asked for.
    pub path: PathBuf,
    pub result: Result<OpenedProject, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_is_named_after_its_folder() {
        assert_eq!(project_name(Path::new("/home/me/code/krampsta")), "krampsta");
        assert_eq!(project_name(Path::new("/")), "/");
    }
}
