//! The left sidebar: brand, new session, the session list and settings.

use krune_core::{AgentStatus, Session};
use rune::{El, Tx, col, icon, row, text};
use rune_kit::shell::RowStatus;
use rune_kit::{Kit, Sidebar, SidebarRow};

use crate::components::icons;
use crate::keys;
use crate::state::{KruneApp, Screen};
use crate::theme;

const NEW_SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘N" } else { "Ctrl+N" };

pub fn view(s: &KruneApp, k: &Kit) -> El {
    let new = SidebarRow::new("New session")
        .icon(&icons::PLUS)
        .detail(NEW_SHORTCUT)
        .height(32.0)
        .on_select(|s: &mut KruneApp, _: &mut Tx| {
            s.new_session();
        })
        .build(k)
        .key(keys::NEW_SESSION);
    let settings = SidebarRow::new("Settings")
        .icon(&icons::GEAR)
        .height(32.0)
        .selected(s.screen == Screen::Settings)
        .on_select(|s: &mut KruneApp, _: &mut Tx| s.screen = Screen::Settings)
        .build(k)
        .key(keys::SETTINGS);
    let sidebar = Sidebar::new()
        .label("Sessions")
        .inset(0.0)
        .header_gap(4.0)
        .header_padding(0.0, 12.0)
        .row_gap(2.0)
        .header(brand(k))
        .header(new)
        .footer(settings);
    s.sessions.iter().fold(sidebar, |sb, x| sb.child(session_row(s, x, k))).build(k)
}

fn brand(k: &Kit) -> El {
    let mark = row()
        .size(24.0, 24.0)
        .radius(7.0)
        .center()
        .fill(theme::accent(k.dark))
        .child(icon(&icons::KRUNE, 16.0).color(k.c.bg_base));
    row().key(keys::BRAND).role("banner").label("Krune").h(44.0).px(6.0).gap(10.0).items_center().child(mark).child(
        col()
            .gap(1.0)
            .min_w(0.0)
            .child(text("Krune").font(k.ty.ui_medium).color(k.c.text_base))
            .child(text("Native AI Coding Agent").font(k.ty.small).color(k.c.text_faint).truncate()),
    )
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

fn session_row(s: &KruneApp, x: &Session, k: &Kit) -> El {
    let id = x.id;
    let mut row = SidebarRow::new(x.title.clone())
        .height(44.0)
        .subtitle(subtitle(x))
        .selected(s.screen == Screen::Chat && s.selected == id)
        .on_select(move |s: &mut KruneApp, _: &mut Tx| s.select(id));
    match x.status {
        AgentStatus::Running => row = row.status(RowStatus::Busy),
        AgentStatus::Failed { .. } => row = row.status(RowStatus::Attention),
        _ => {}
    }
    row.build(k).key(keys::session(id))
}

#[cfg(test)]
mod tests {
    use super::plain;

    #[test]
    fn previews_drop_markdown_markers() {
        assert_eq!(plain("Welcome to **Krune**, `cargo run`"), "Welcome to Krune, cargo run");
        assert_eq!(plain("## Heading"), "Heading");
    }
}
