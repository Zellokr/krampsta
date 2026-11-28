//! Commands' sandbox. On Linux, `run_command` starts its shell from a thread confined with
//! Landlock, which the shell and everything it starts inherit: they read and run anything, but
//! write only in the project, its Cargo build folder, the temporary folders, `/dev` and the
//! usual tool caches (`~/.cargo`, `~/.rustup`, `~/.cache`, `~/.npm`), plus what the user adds in
//! `permissions.json`. No `pre_exec` (that would need `unsafe`): the restriction is applied to
//! a thread made for the spawn, which then ends. It also sets `no_new_privs`, so `sudo` and other
//! setuid programs can't raise privileges inside it.
//!
//! A kernel without Landlock runs the command unconfined, and `commands.log` says so. Other
//! systems have no sandbox yet.
//!
//! ```json
//! { "sandbox": { "enabled": true, "writable": ["~/.gradle", "/opt/tools/state"] } }
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The user's sandbox settings (`sandbox` in `permissions.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRules {
    /// On unless the user turns it off.
    #[serde(default = "on")]
    pub enabled: bool,
    /// More folders commands may write in (`~` is the home folder).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writable: Vec<String>,
}

fn on() -> bool {
    true
}

impl Default for SandboxRules {
    fn default() -> Self {
        SandboxRules { enabled: true, writable: Vec::new() }
    }
}

impl SandboxRules {
    pub fn is_default(&self) -> bool {
        *self == SandboxRules::default()
    }
}

/// How confined a command ran, for `commands.log`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confinement {
    /// Every restriction asked for holds.
    Full,
    /// The kernel enforces only part of it (an older Landlock).
    Partial,
    /// The kernel can't confine it; it ran without.
    Unavailable,
    /// The user turned the sandbox off, or this system has none.
    Off,
}

impl Confinement {
    pub fn describe(&self) -> &'static str {
        match self {
            Confinement::Full => "on",
            Confinement::Partial => "partial",
            Confinement::Unavailable => "unavailable on this kernel",
            Confinement::Off => "off",
        }
    }
}

/// Where a command may write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sandbox {
    writable: Vec<PathBuf>,
}

impl Sandbox {
    /// The sandbox for a command in `root` (and building in `cargo_target`), or `None` when
    /// the user turned it off or this system has none.
    pub fn for_command(root: &Path, cargo_target: Option<&Path>, rules: &SandboxRules) -> Option<Sandbox> {
        if !rules.enabled || !cfg!(target_os = "linux") {
            return None;
        }
        let home = dirs::home_dir();
        let mut writable = vec![root.to_owned(), std::env::temp_dir(), "/tmp".into(), "/var/tmp".into(), "/dev".into()];
        writable.extend(cargo_target.map(Path::to_owned));
        writable.extend(dirs::cache_dir());
        if let Some(home) = &home {
            writable.extend([".cargo", ".rustup", ".npm"].map(|d| home.join(d)));
        }
        writable.extend(rules.writable.iter().map(|w| expand(w, home.as_deref())));
        Some(Sandbox::writing_in(writable))
    }

    /// A sandbox that writes only in `writable`.
    pub fn writing_in(writable: Vec<PathBuf>) -> Sandbox {
        Sandbox { writable }
    }

    /// Runs `spawn` (which starts the command) confined, on a thread made for it.
    pub fn spawn<T: Send>(&self, spawn: impl FnOnce() -> T + Send) -> Result<(T, Confinement), String> {
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let confinement = confine(&self.writable);
                    (spawn(), confinement)
                })
                .join()
                .map_err(|_| "the sandboxed start of the command failed".to_owned())
        })
    }
}

/// `~/x` as a path in the home folder.
fn expand(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// Confines the calling thread (and what it starts from now on): read and run everywhere,
/// write only beneath `writable` (the folders that exist).
#[cfg(target_os = "linux")]
fn confine(writable: &[PathBuf]) -> Confinement {
    use landlock::{
        ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, path_beneath_rules,
    };

    let abi = ABI::V5;
    let existing: Vec<&PathBuf> = writable.iter().filter(|p| p.exists()).collect();
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .and_then(|r| r.create())
        .and_then(|r| r.add_rules(path_beneath_rules(["/"], AccessFs::from_read(abi))))
        .and_then(|r| r.add_rules(path_beneath_rules(existing, AccessFs::from_all(abi))))
        .and_then(|r| r.restrict_self());
    match status.map(|s| s.ruleset) {
        Ok(RulesetStatus::FullyEnforced) => Confinement::Full,
        Ok(RulesetStatus::PartiallyEnforced) => Confinement::Partial,
        Ok(RulesetStatus::NotEnforced) | Err(_) => Confinement::Unavailable,
    }
}

#[cfg(not(target_os = "linux"))]
fn confine(_: &[PathBuf]) -> Confinement {
    Confinement::Off
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_default_to_on_and_read_from_json() {
        let rules: SandboxRules = serde_json::from_str("{}").unwrap();
        assert!(rules.enabled && rules.is_default());
        let off: SandboxRules = serde_json::from_str(r#"{"enabled": false, "writable": ["~/.gradle"]}"#).unwrap();
        assert!(!off.enabled);
        assert_eq!(expand("~/.gradle", Some(Path::new("/home/me"))), PathBuf::from("/home/me/.gradle"));
        assert_eq!(expand("/opt/x", Some(Path::new("/home/me"))), PathBuf::from("/opt/x"));
    }

    #[test]
    fn turned_off_there_is_no_sandbox() {
        let off = SandboxRules { enabled: false, writable: Vec::new() };
        assert_eq!(Sandbox::for_command(Path::new("/p"), None, &off), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn writes_outside_the_writable_folders_fail_and_inside_succeed() {
        let (project, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let sandbox = Sandbox::writing_in(vec![project.path().to_owned()]);
        let (inside, outside_file) = (project.path().join("in.txt"), outside.path().join("out.txt"));
        let ((ok, denied), confinement) =
            sandbox.spawn(|| (std::fs::write(&inside, "yes"), std::fs::write(&outside_file, "no"))).unwrap();
        if confinement == Confinement::Unavailable {
            eprintln!("Landlock is not available here; skipping");
            return;
        }
        assert!(ok.is_ok(), "{ok:?}");
        assert_eq!(denied.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!outside_file.exists());
        assert!(std::fs::write(outside.path().join("later.txt"), "ok").is_ok(), "only the sandbox thread is confined");
    }
}
