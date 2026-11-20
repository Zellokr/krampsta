//! A per-call snapshot of proposals. File bodies stay shared with the review panel.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use krampsta_agent::CancellationToken;

use crate::edit::Pending;
use crate::project::Project;

pub struct Overlay(BTreeMap<PathBuf, Arc<str>>);

impl Overlay {
    pub fn new(project: &Project, pending: &Pending, cancel: &CancellationToken) -> Result<Self, String> {
        let mut files = BTreeMap::new();
        for (path, content) in pending.snapshot() {
            if cancel.is_cancelled() {
                return Err("stopped".into());
            }
            if let Ok(real) = project.resolve_for_write(&path)
                && !real.is_dir()
                && project.show(&real) == path
            {
                files.insert(real, content);
            }
        }
        Ok(Self(files))
    }

    pub fn resolve(&self, project: &Project, asked: &str) -> Result<PathBuf, String> {
        project.resolve(asked).or_else(|err| {
            let path = project.resolve_for_write(asked).map_err(|_| err.clone())?;
            if self.0.keys().any(|file| file.starts_with(&path)) { Ok(path) } else { Err(err) }
        })
    }

    pub fn get(&self, file: &Path) -> Option<&str> {
        self.0.get(file).map(AsRef::as_ref)
    }

    pub fn under<'a>(&'a self, base: &'a Path) -> impl Iterator<Item = (&'a Path, &'a str)> {
        self.0
            .range(base.to_owned()..)
            .take_while(move |(path, _)| path.starts_with(base))
            .map(|(p, s)| (p.as_path(), s.as_ref()))
    }
}

/// Ignore rules for paths the disk walker cannot visit. Walk each component so that a child
/// cannot unignore itself inside an excluded directory. Only on-disk ignore files take effect.
pub struct VirtualIgnores<'a> {
    root: &'a Path,
    rules: BTreeMap<PathBuf, [Gitignore; 2]>,
    global: Gitignore,
    exclude: Gitignore,
}

impl<'a> VirtualIgnores<'a> {
    pub fn new(root: &'a Path) -> Self {
        let global = GitignoreBuilder::new(root).build_global().0;
        let mut exclude = GitignoreBuilder::new(root);
        exclude.add(root.join(".git/info/exclude"));
        Self { root, rules: BTreeMap::new(), global, exclude: exclude.build().unwrap_or_else(|_| Gitignore::empty()) }
    }

    pub fn skips(&mut self, path: &Path, base: &Path, is_dir: bool, whitelisted: bool) -> bool {
        for entry in path.ancestors().take_while(|p| *p != base) {
            if entry.file_name().is_some_and(|name| name == ".git") {
                return true;
            }
            if entry == path && whitelisted {
                continue;
            }
            let is_dir = is_dir || entry != path;
            let mut matches = [const { ignore::Match::None }; 2];
            for dir in entry.parent().into_iter().flat_map(Path::ancestors).take_while(|d| d.starts_with(self.root)) {
                let rules = self.rules.entry(dir.to_owned()).or_insert_with(|| {
                    [".ignore", ".gitignore"].map(|name| {
                        let mut builder = GitignoreBuilder::new(dir);
                        builder.add(dir.join(name));
                        builder.build().unwrap_or_else(|_| Gitignore::empty())
                    })
                });
                for (matched, rule) in matches.iter_mut().zip(rules) {
                    if matched.is_none() {
                        *matched = rule.matched(entry, is_dir).map(|_| ());
                    }
                }
            }
            let [local, git] = matches;
            let matched = local
                .or(git)
                .or(self.exclude.matched(entry, is_dir).map(|_| ()))
                .or(self.global.matched(entry, is_dir).map(|_| ()));
            if matched.is_ignore() {
                return true;
            }
        }
        false
    }
}
