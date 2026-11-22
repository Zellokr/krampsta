//! The left sidebar: brand, project folder, new session, the sessions by project and settings.

use std::path::Path;

use krampsta_agent::project_name;
use krampsta_core::{AgentStatus, Session};
use rune::{El, Tx, col, icon, row, text};
use rune_kit::shell::RowStatus;
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit, Sidebar, SidebarRow, SidebarSection};

use crate::components::{icons, menus};
use crate::keys;
use crate::projects::Group;
use crate::state::{KrampstaApp, Screen};
use crate::theme;

const NEW_SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘N" } else { "Ctrl+N" };
const OPEN_SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘O" } else { "Ctrl+O" };

pub fn view(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>) -> El {
    let new = SidebarRow::new("New session")
        .icon(&icons::PLUS)
        .detail(NEW_SHORTCUT)
        .height(32.0)
        .on_select(|s: &mut KrampstaApp, _: &mut Tx| {
            s.new_session();
        })
        .build(k)
        .key(keys::NEW_SESSION);
    let settings = SidebarRow::new("Settings")
        .icon(&icons::GEAR)
        .height(32.0)
        .selected(s.screen == Screen::Settings)
        .on_select(|s: &mut KrampstaApp, _: &mut Tx| s.open_settings())
        .build(k)
        .key(keys::SETTINGS);
    let sidebar = Sidebar::new()
        .label("Sessions")
        .inset(0.0)
        .header_gap(4.0)
        .header_padding(0.0, 12.0)
        .row_gap(2.0)
        .header(brand(k))
        .header(project(s, k))
        .header(new)
        .footer(settings);
    s.groups().into_iter().fold(sidebar, |sb, g| sb.section(group(s, g, k, layer))).build(k)
}

/// One project's sessions under its name, folding away on a press. Another project's group
/// carries a button that opens its folder.
fn group(s: &KrampstaApp, g: Group<'_>, k: &Kit, layer: &mut Vec<El>) -> SidebarSection {
    let root = g.root.map(Path::to_path_buf);
    let is_open = s.project.as_ref().map(|p| p.root.as_path()) == g.root;
    let toggle_root = root.clone();
    let mut section = SidebarSection::new(g.name.clone())
        .key(keys::project_group(g.root))
        .count(g.sessions.len())
        .collapsed(s.is_group_collapsed(g.root))
        .on_toggle(move |s: &mut KrampstaApp, _: &mut Tx| s.toggle_group(toggle_root.clone()));
    if let Some(folder) = root.filter(|_| !is_open) {
        let open = IconButton::new(&icons::FOLDER)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label(format!("Open {}", g.name))
            .help(format!("Open {}", g.name))
            .on_press(move |s: &mut KrampstaApp| s.open_project(folder.clone()))
            .build(k)
            .key(keys::open_group(g.root));
        section = section.action(open);
    }
    section.rows(g.sessions.into_iter().map(|x| session_row(s, x, k, layer)))
}

fn brand(k: &Kit) -> El {
    let mark = row()
        .size(24.0, 24.0)
        .radius(7.0)
        .center()
        .fill(theme::accent(k.dark))
        .child(icon(&icons::KRAMPSTA, 16.0).color(k.c.bg_base));
    row().key(keys::BRAND).role("banner").label("Krampsta").h(44.0).px(6.0).gap(10.0).items_center().child(mark).child(
        col()
            .gap(1.0)
            .min_w(0.0)
            .child(text("Krampsta").font(k.ty.ui_medium).color(k.c.text_base))
            .child(text("Native AI Coding Agent").font(k.ty.small).color(k.c.text_faint).truncate()),
    )
}

/// The project folder: its name and where it is. Pressing it opens another.
fn project(s: &KrampstaApp, k: &Kit) -> El {
    let (title, subtitle) = match (&s.project, &s.opening_project) {
        (_, Some(path)) => (project_name(path), "Opening…".to_owned()),
        (Some(p), None) => (p.name.clone(), p.display_root()),
        (None, None) => ("Open a folder".to_owned(), "The agent has no tools".to_owned()),
    };
    let mut row = SidebarRow::new(title);
    if s.opening_project.is_some() {
        row = row.status(RowStatus::Busy);
    }
    row.icon(&icons::FOLDER)
        .subtitle(subtitle)
        .detail(OPEN_SHORTCUT)
        .height(44.0)
        .on_select(|s: &mut KrampstaApp, tx: &mut Tx| s.choose_project(tx))
        .build(k)
        .key(keys::PROJECT)
        .label("Project folder")
}

fn subtitle(x: &Session) -> String {
    match &x.status {
        AgentStatus::Running => "Running…".to_owned(),
        AgentStatus::Failed { .. } => "Failed".to_owned(),
        AgentStatus::Cancelled => "Cancelled".to_owned(),
        AgentStatus::Idle | AgentStatus::Completed => x.preview().map_or_else(|| "No messages yet".to_owned(), plain),
    }
}

/// A Markdown line as plain text for a one-line preview: emphasis and code markers dropped.
fn plain(line: &str) -> String {
    line.trim_start_matches(['#', '>', '-', ' ']).chars().filter(|c| !matches!(c, '*' | '`' | '_')).collect()
}

fn session_row(s: &KrampstaApp, x: &Session, k: &Kit, layer: &mut Vec<El>) -> El {
    let id = x.id;
    let mut row = SidebarRow::new(x.title.clone())
        .height(44.0)
        .subtitle(subtitle(x))
        .selected(s.screen == Screen::Chat && s.selected == id)
        .on_select(move |s: &mut KrampstaApp, _: &mut Tx| s.select(id))
        .on_double_click(move |s: &mut KrampstaApp, _: &mut Tx| s.start_rename(id));
    if let Some(edit) = s.renaming_field(id) {
        row = row.renaming(
            edit,
            |s: &mut KrampstaApp| &mut s.renaming.edit,
            |s: &mut KrampstaApp, _: &mut Tx| s.commit_rename(),
            |s: &mut KrampstaApp, _: &mut Tx| s.cancel_rename(),
        );
    }
    match x.status {
        AgentStatus::Running => row = row.status(RowStatus::Busy),
        AgentStatus::Failed { .. } => row = row.status(RowStatus::Attention),
        _ => {}
    }
    menus::session(s, x, row.build(k).key(keys::session(id)), k, layer)
}

#[cfg(test)]
mod tests {
    use super::plain;

    #[test]
    fn previews_drop_markdown_markers() {
        assert_eq!(plain("Welcome to **Krampsta**, `cargo run`"), "Welcome to Krampsta, cargo run");
        assert_eq!(plain("## Heading"), "Heading");
    }
}
