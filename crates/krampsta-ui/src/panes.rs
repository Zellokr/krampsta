//! Two sessions side by side. The selected session is always the active pane's, so the sidebar,
//! the tabs and the status pill keep meaning what they did; `split` holds the other pane's
//! session. Pressing a pane's background, typing in its composer or picking its session makes it
//! the active one.

use krampsta_core::SessionId;
use rune::{El, col, el, row, text};
use rune_kit::{IconButton, IconButtonVariant, Kit};

use crate::components::icons;
use crate::keys;
use crate::state::KrampstaApp;
use crate::views::chat;

/// The second pane: whose session it shows, and which side the active one is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Split {
    /// The session in the pane that isn't active.
    pub other: SessionId,
    /// Whether the active pane is the right one.
    pub active_right: bool,
}

impl KrampstaApp {
    /// The panes on screen, left to right, each with whether it's the active one.
    pub fn panes(&self) -> Vec<(SessionId, bool)> {
        match self.split {
            None => vec![(self.selected, true)],
            Some(Split { other, active_right: false }) => vec![(self.selected, true), (other, false)],
            Some(Split { other, active_right: true }) => vec![(other, false), (self.selected, true)],
        }
    }

    /// Opens a second pane beside the active one, with the tab after it (or before it, or
    /// another session, or a new one), or closes it, leaving the active pane.
    pub fn toggle_split(&mut self) {
        if self.split.take().is_some() {
            self.refit();
            return;
        }
        let at = self.tabs.iter().position(|t| *t == self.selected).unwrap_or(0);
        let neighbour = self.tabs.get(at + 1).or_else(|| at.checked_sub(1).and_then(|i| self.tabs.get(i))).copied();
        let other = neighbour.or_else(|| self.sessions.iter().map(|s| s.id).find(|id| *id != self.selected));
        let other = match other {
            Some(id) => id,
            None => {
                // Nothing else to show: a new session, beside the one that stays active.
                let active = self.selected;
                let id = self.new_session();
                self.select(active);
                id
            }
        };
        self.open_tab(other);
        self.split = Some(Split { other, active_right: false });
        // Two conversations need more room: a side pane may close to make it.
        self.refit();
    }

    /// Makes the pane showing `id` the active one (or shows `id` in the active pane).
    pub fn focus_session(&mut self, id: SessionId) {
        if id != self.selected {
            self.select(id);
        }
    }

    /// `select`'s part for a split: picking the other pane's session swaps which pane is active,
    /// instead of showing the session twice.
    pub(crate) fn swap_if_other(&mut self, id: SessionId) {
        if let Some(split) = &mut self.split
            && split.other == id
        {
            split.other = self.selected;
            split.active_right = !split.active_right;
        }
    }

    /// Closes the split if `id` (closed or deleted) was in its other pane.
    pub(crate) fn forget_pane(&mut self, id: SessionId) {
        if self.split.is_some_and(|s| s.other == id) {
            self.split = None;
        }
    }
}

/// The conversation area: one pane, or two side by side with a hairline between them.
pub fn view(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>) -> El {
    let panes = s.panes();
    if panes.len() == 1 {
        return pane(s, panes[0].0, true, false, 0, k, layer);
    }
    let mut area = row().key(keys::PANES).w_full().h_full().min_h(0.0);
    for (i, (id, active)) in panes.into_iter().enumerate() {
        if i > 0 {
            area = area.child(el().w(1.0).h_full().fill(k.c.border_weak));
        }
        area = area.child(pane(s, id, active, true, i, k, layer));
    }
    area
}

/// One pane: when split, a header names its session (the inactive one dimmed) and closes the
/// split; a press on its background makes it the active one.
fn pane(s: &KrampstaApp, id: SessionId, active: bool, split: bool, index: usize, k: &Kit, layer: &mut Vec<El>) -> El {
    let body = chat::view(s, id, k, layer);
    let mut pane = col().key(keys::pane(index)).grow(1.0).w_full().h_full().min_w(0.0).min_h(0.0);
    if split {
        let title = s.session(id).map_or("", |x| x.title.as_str()).to_owned();
        let ink = if active { k.c.text_base } else { k.c.text_faint };
        let close = IconButton::new(&icons::XMARK)
            .variant(IconButtonVariant::GhostMuted)
            .label("Close the split")
            .help(if cfg!(target_os = "macos") { "Close the split (⌘\\)" } else { "Close the split (Ctrl+\\)" })
            .on_press(|s: &mut KrampstaApp| s.toggle_split())
            .build(k)
            .key(keys::pane_close(index));
        let header = row()
            .key(keys::pane_header(index))
            .w_full()
            .h(32.0)
            .px(16.0)
            .gap(8.0)
            .items_center()
            .child(text(title).font(k.ty.small_medium).color(ink).truncate().shrink().min_w(0.0))
            .child(el().grow(1.0))
            .child(close);
        pane = pane.child(header);
    }
    // The conversation takes what the header leaves (a full-height body would push the
    // composer's last line out of the window).
    let body = col().grow(1.0).w_full().min_h(0.0).child(body);
    pane.child(body).on_background_press(move |s: &mut KrampstaApp| s.focus_session(id))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::{MockCatalog, MockConfig};

    use super::*;

    fn app() -> KrampstaApp {
        KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant()))).with_sample_sessions()
    }

    #[test]
    fn splitting_shows_another_session_beside_the_active_one_and_closing_keeps_it() {
        let mut app = app();
        let (a, b) = (app.sessions[0].id, app.sessions[1].id);
        app.toggle_split();
        assert_eq!(app.panes(), [(a, true), (b, false)], "another session on the right");
        assert!(app.tabs.contains(&b), "with its tab");
        assert_eq!(app.visible_sessions(), [a, b], "both stream");
        app.toggle_split();
        assert_eq!(app.panes(), [(a, true)]);
    }

    #[test]
    fn picking_the_other_panes_session_makes_that_pane_active() {
        let mut app = app();
        let (a, b) = (app.sessions[0].id, app.sessions[1].id);
        app.toggle_split();
        app.focus_session(b);
        assert_eq!(app.panes(), [(a, false), (b, true)], "same places, the right one active");
        assert_eq!(app.selected, b);
        let c = app.new_session();
        assert_eq!(app.panes(), [(a, false), (c, true)], "the active pane shows what's picked");
    }

    #[test]
    fn a_split_with_nothing_else_to_show_makes_a_new_session() {
        let mut app = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())));
        let only = app.selected;
        app.toggle_split();
        let panes = app.panes();
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0], (only, true));
        assert_ne!(panes[1].0, only);
    }

    #[test]
    fn closing_or_deleting_the_other_panes_session_closes_the_split() {
        let mut app = app();
        let b = app.sessions[1].id;
        app.toggle_split();
        app.close_tab(b);
        assert_eq!(app.split, None);
        app.toggle_split();
        let other = app.split.map(|s| s.other).unwrap();
        app.ask_delete(other);
        app.confirm_delete();
        assert_eq!(app.split, None);
    }
}
