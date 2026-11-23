//! What the project tells its agent, read once when it is opened: the rules in its root
//! `AGENTS.md`, and the skills it offers (`.agents/skills/*/SKILL.md`, and `.claude/skills/`),
//! listed by name and description only. A skill's guide is read when a task needs it, with
//! `read_file`, so the model never carries every guide on every request.
//!
//! None of it grants anything: commands still pass the user's rules, and edits still wait for
//! review. It is text for the model, nothing more.

use std::fmt::Write as _;
use std::io::Read as _;

use crate::project::Project;

/// The rules file at the project's root.
pub const RULES: &str = "AGENTS.md";
/// Most of `AGENTS.md` sent with each request (it goes with every one): the rest is cut, with a note.
pub const MAX_RULES: usize = 32 * 1024;
/// Where skills live, each in a folder of its own with its `SKILL.md`.
pub const SKILL_DIRS: [&str; 2] = [".agents/skills", ".claude/skills"];
/// Most skills listed; a project with more is unusual, and the list goes with every request.
pub const MAX_SKILLS: usize = 64;
/// Most characters of a skill's description in the list.
const MAX_DESCRIPTION: usize = 1024;
/// Most bytes read of a `SKILL.md` to find its header (name and description come first).
const MAX_HEADER: u64 = 16 * 1024;

/// A skill, as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Its guide, as the tools take it (`.agents/skills/verify-ui/SKILL.md`).
    pub path: String,
}

/// The project's guidance for the model, or `None` when it has neither rules nor skills. Blocks
/// (it reads files): called when the project is opened, off the UI thread.
pub fn read(project: &Project) -> Option<String> {
    let rules = read_rules(project);
    let skills = find_skills(project);
    compose(rules.as_deref(), &skills)
}

/// The text the model gets.
pub fn compose(rules: Option<&str>, skills: &[Skill]) -> Option<String> {
    let rules = rules.map(str::trim).filter(|r| !r.is_empty());
    if rules.is_none() && skills.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(rules.map_or(0, str::len) + 512 + skills.len() * 256);
    if let Some(rules) = rules {
        out.push_str(
            "The project's own instructions, from its AGENTS.md. Follow its conventions for work in \
             this project, over your general habits, as far as they fit Krampsta's rules and the \
             user's permissions: they don't let you run commands or apply changes without the user, \
             which the tools still ask for.\n\n<project_instructions>\n",
        );
        out.push_str(rules);
        out.push_str("\n</project_instructions>");
    }
    if !skills.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(
            "The project's skills: guides for particular tasks. When a task is one a skill \
             covers, or the user names it, read its guide with read_file first and follow it. A \
             skill may say which command to run; the user still allows it.",
        );
        for s in skills {
            let _ = write!(out, "\n- {} ({}): {}", s.name, s.path, s.description);
        }
    }
    Some(out)
}

/// `AGENTS.md` at the root, cut at [`MAX_RULES`]. `None` when there is none or it can't be read
/// (said on stderr: the project still opens).
fn read_rules(project: &Project) -> Option<String> {
    if !project.root().join(RULES).is_file() {
        return None;
    }
    let read = project.resolve(RULES).and_then(|path| {
        let mut bytes = Vec::with_capacity(MAX_RULES + 1);
        std::fs::File::open(&path)
            .and_then(|f| f.take(MAX_RULES as u64 + 1).read_to_end(&mut bytes))
            .map_err(|e| format!("could not read {RULES}: {e}"))?;
        Ok(bytes)
    });
    match read {
        Ok(bytes) => Some(rules_text(bytes)),
        Err(e) => {
            eprintln!("krampsta: {e}");
            None
        }
    }
}

/// The rules as text: invalid UTF-8 replaced, and past [`MAX_RULES`] cut at a character, with a note.
fn rules_text(mut bytes: Vec<u8>) -> String {
    let cut = bytes.len() > MAX_RULES;
    if cut {
        bytes.truncate(MAX_RULES);
        // Don't leave half a character at the end.
        let valid = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => bytes.len(),
        };
        bytes.truncate(valid);
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if cut {
        let _ =
            write!(text, "\n\n(Cut: {RULES} is longer than {} KB. Read the rest with read_file.)", MAX_RULES / 1024);
    }
    text
}

/// Every skill under [`SKILL_DIRS`], sorted by name; the first of a name wins (`.agents` first).
fn find_skills(project: &Project) -> Vec<Skill> {
    let mut skills: Vec<Skill> = Vec::new();
    for dir in SKILL_DIRS {
        let Ok(entries) = std::fs::read_dir(project.root().join(dir)) else { continue };
        let mut folders: Vec<_> = entries.filter_map(Result::ok).map(|e| e.file_name()).collect();
        folders.sort();
        for folder in folders {
            if skills.len() >= MAX_SKILLS {
                break;
            }
            let path = format!("{dir}/{}/SKILL.md", folder.to_string_lossy());
            let Some(skill) = read_skill(project, &path) else { continue };
            if !skills.iter().any(|s| s.name == skill.name) {
                skills.push(skill);
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// The skill whose guide is at `path`, or `None` when there is no guide or no description
/// (the model couldn't tell when to use it). A guide that can't be read is said on stderr.
fn read_skill(project: &Project, path: &str) -> Option<Skill> {
    if !project.root().join(path).is_file() {
        return None;
    }
    let header = project.resolve(path).and_then(|real| {
        let mut bytes = Vec::new();
        std::fs::File::open(&real)
            .and_then(|f| f.take(MAX_HEADER).read_to_end(&mut bytes))
            .map_err(|e| format!("could not read {path}: {e}"))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    });
    let header = match header {
        Ok(header) => header,
        Err(e) => {
            eprintln!("krampsta: {e}");
            return None;
        }
    };
    let (name, description) = front_matter(&header);
    let folder = path.rsplit('/').nth(1).unwrap_or(path);
    let name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| folder.to_owned());
    let description = description.filter(|d| !d.is_empty())?;
    let mut short = String::new();
    crate::read::push_cut(&mut short, &description, MAX_DESCRIPTION);
    Some(Skill { name, description: short, path: path.to_owned() })
}

/// `name` and `description` from a guide's YAML header (`---` … `---`): plain or quoted values on
/// one line, or a folded / literal block (`>`, `|`) on the indented lines below.
fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return (None, None);
    }
    let (mut name, mut description) = (None, None);
    let mut block: Option<(&str, String)> = None;
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        if let Some((_, text)) = block.as_mut()
            && (line.starts_with([' ', '\t']) || line.trim().is_empty())
        {
            if !text.is_empty() && !line.trim().is_empty() {
                text.push(' ');
            }
            text.push_str(line.trim());
            continue;
        }
        if let Some((key, text)) = block.take() {
            set(key, text.trim().to_owned(), &mut name, &mut description);
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let (key, value) = (key.trim(), value.trim());
        if !matches!(key, "name" | "description") {
            continue;
        }
        if value.starts_with(['>', '|']) {
            block = Some((key, String::new()));
        } else {
            set(key, unquote(value).to_owned(), &mut name, &mut description);
        }
    }
    if let Some((key, text)) = block {
        set(key, text.trim().to_owned(), &mut name, &mut description);
    }
    (name, description)
}

fn set(key: &str, value: String, name: &mut Option<String>, description: &mut Option<String>) {
    match key {
        "name" => *name = Some(value),
        _ => *description = Some(value),
    }
}

fn unquote(value: &str) -> &str {
    for q in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(q).and_then(|v| v.strip_suffix(q)) {
            return inner;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().expect("a temporary folder");
        for (name, body) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().expect("files sit in a folder")).expect("its folder");
            std::fs::write(path, body).expect("the file");
        }
        let p = Project::open(dir.path()).expect("the folder opens");
        (dir, p)
    }

    #[test]
    fn a_project_with_neither_rules_nor_skills_adds_nothing() {
        let (_dir, p) = project(&[("README.md", "hi")]);
        assert_eq!(read(&p), None);
    }

    #[test]
    fn the_root_rules_are_sent_whole_and_marked_as_the_projects() {
        let (_dir, p) = project(&[("AGENTS.md", "Run `make check` before you finish.\n")]);
        let text = read(&p).expect("there are rules");
        assert!(text.contains("<project_instructions>\nRun `make check` before you finish.\n</project_instructions>"));
        assert!(text.contains("don't let you run commands"), "the rules grant nothing: {text}");
        assert!(!text.contains("skills"), "no skills, no list: {text}");
    }

    #[test]
    fn rules_in_a_subfolder_are_not_read_yet() {
        let (_dir, p) = project(&[("web/AGENTS.md", "Use pnpm.")]);
        assert_eq!(read(&p), None);
    }

    #[test]
    fn long_rules_are_cut_at_a_character_with_a_note() {
        let long = "é".repeat(MAX_RULES); // two bytes each: twice the limit
        let text = rules_text(long.into_bytes());
        assert!(text.starts_with("éé"));
        assert!(!text.contains('\u{FFFD}'), "no half character at the cut");
        assert!(text.ends_with("Read the rest with read_file.)"), "{}", &text[text.len() - 80..]);
        assert!(text.len() < MAX_RULES + 200);
    }

    #[test]
    fn skills_are_listed_by_name_description_and_path_but_not_their_guides() {
        let (_dir, p) = project(&[
            (
                ".agents/skills/validate/SKILL.md",
                "---\nname: validate\ndescription: Check a fix with `just ci`.\n---\n\n# Validate\n\nSECRET STEPS",
            ),
            (
                ".claude/skills/verify-ui/SKILL.md",
                "---\nname: verify-ui\ndescription: \"Look at the screenshots.\"\n---\nBODY",
            ),
            (".agents/skills/empty/SKILL.md", "---\nname: empty\n---\nno description: never offered"),
            (".agents/skills/notes/README.md", "not a skill"),
        ]);
        let text = read(&p).expect("there are skills");
        assert!(text.contains("- validate (.agents/skills/validate/SKILL.md): Check a fix with `just ci`."), "{text}");
        assert!(text.contains("- verify-ui (.claude/skills/verify-ui/SKILL.md): Look at the screenshots."), "{text}");
        assert!(!text.contains("SECRET STEPS") && !text.contains("BODY"), "guides are read on demand: {text}");
        assert!(!text.contains("empty") && !text.contains("notes"), "{text}");
        assert!(!text.contains("project_instructions"), "no AGENTS.md, no rules block: {text}");
    }

    #[test]
    fn a_skill_named_twice_is_listed_once_from_agents_first() {
        let (_dir, p) = project(&[
            (".agents/skills/check/SKILL.md", "---\nname: check\ndescription: the agents one\n---\n"),
            (".claude/skills/check/SKILL.md", "---\nname: check\ndescription: the claude one\n---\n"),
        ]);
        let skills = find_skills(&p);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "the agents one");
    }

    #[test]
    fn a_skill_without_a_name_takes_its_folders() {
        let (_dir, p) = project(&[(".agents/skills/release/SKILL.md", "---\ndescription: Cut a release.\n---\n")]);
        assert_eq!(find_skills(&p)[0].name, "release");
    }

    #[test]
    fn the_header_takes_quoted_values_and_folded_blocks() {
        let (name, description) = front_matter(
            "---\nname: 'deploy'\ndescription: >\n  Ship it to staging,\n  then to prod.\nlicense: MIT\n---\n",
        );
        assert_eq!(name.as_deref(), Some("deploy"));
        assert_eq!(description.as_deref(), Some("Ship it to staging, then to prod."));
        assert_eq!(front_matter("# No header\nname: x"), (None, None));
    }
}
