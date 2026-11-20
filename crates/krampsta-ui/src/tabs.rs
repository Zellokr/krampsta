//! Sessions open in tabs above the conversation (rune-kit's `TabStrip`): the selected session is
//! the active tab. Picking a session in the sidebar opens its tab; closing a tab leaves the
//! session in the sidebar. The last tab stays: there is always a conversation on screen.

use krampsta_core::{AgentStatus, SessionId};
use rune::{Cx, El, Tx};
use rune_kit::Kit;
use rune_kit::shell::{TabItem, TabItemStatus, TabOrientation, TabReorder, TabStrip};

use crate::components::icons;
use crate::keys;
use crate::state::KrampstaApp;

impl KrampstaApp {
    /// Opens `id`'s tab (after the active one) if it isn't open.
    pub(crate) fn open_tab(&mut self, id: SessionId) {
        if self.tabs.contains(&id) {
            return;
        }
        let at = self.tabs.iter().position(|t| *t == self.selected).map_or(self.tabs.len(), |i| i + 1);
        self.tabs.insert(at, id);
    }

    /// Closes `id`'s tab; closing the active one selects its neighbour. The last tab stays.
    pub fn close_tab(&mut self, id: SessionId) {
        if self.tabs.len() <= 1 {
            return;
        }
        let Some(at) = self.tabs.iter().position(|t| *t == id) else { return };
        self.tabs.remove(at);
        self.forget_pane(id);
        // Split: closing the active pane's tab leaves the other pane, whole.
        if self.selected == id
            && let Some(split) = self.split.take()
        {
            self.select(split.other);
            return;
        }
        if self.selected == id {
            let next = self.tabs[at.min(self.tabs.len() - 1)];
            self.select(next);
        }
    }

    /// Forgets the tab of a session that went away (deleted).
    pub(crate) fn drop_tab(&mut self, id: SessionId) {
        self.tabs.retain(|t| *t != id);
    }

    fn move_tab(&mut self, moved: &TabReorder) {
        if moved.from < self.tabs.len() && moved.to < self.tabs.len() {
            let id = self.tabs.remove(moved.from);
            self.tabs.insert(moved.to, id);
        }
    }
}

/// What a tab's status slot shows for a session.
fn status(s: &KrampstaApp, id: SessionId, status: &AgentStatus) -> Option<TabItemStatus> {
    let asking = s.approvals.iter().any(|a| s.session(id).and_then(|x| x.active_run()) == Some(a.run));
    match status {
        AgentStatus::Running if asking => Some(TabItemStatus::Attention),
        AgentStatus::Running => Some(TabItemStatus::Busy),
        AgentStatus::Failed { .. } => Some(TabItemStatus::Failed),
        _ => None,
    }
}

/// The tab strip, filling the toolbar's leading side.
pub fn strip(s: &KrampstaApp, cx: &mut Cx<KrampstaApp>, k: &Kit, layer: &mut Vec<El>) -> El {
    // Each tab's id, owned for the frame (the strip borrows them).
    let ids: Vec<(SessionId, String)> = s.tabs.iter().map(|id| (*id, keys::tab(*id))).collect();
    let active = keys::tab(s.selected);
    let closable = s.tabs.len() > 1;
    let items = ids.iter().filter_map(|(id, tab)| {
        let session = s.session(*id)?;
        let item = TabItem::new(tab.as_str(), session.title.as_str())
            .icon(&icons::SESSION)
            .status(status(s, *id, &session.status))
            .closable(closable);
        Some(item)
    });
    let strip = TabStrip::new(keys::TABS, TabOrientation::Horizontal)
        .tabs(items)
        .active(Some(active.as_str()))
        .viewport_width(cx.layout_size(keys::TABS).map(|z| z[0]))
        .drag_state(&s.tab_drag, |s: &mut KrampstaApp| &mut s.tab_drag)
        .close_icon(&icons::XMARK)
        // It sits on the chat panel, not the window: the close slot paints the panel colour.
        .fill(k.c.bg_base)
        .label("Open sessions")
        .on_select(|s: &mut KrampstaApp, tab: &str, _: &mut Tx| {
            if let Some(id) = keys::tab_session(tab) {
                s.select(id);
            }
        })
        .on_close(|s: &mut KrampstaApp, tab: &str, _: &mut Tx| {
            if let Some(id) = keys::tab_session(tab) {
                s.close_tab(id);
            }
        })
        .on_reorder(|s: &mut KrampstaApp, moved: TabReorder, _: &mut Tx| s.move_tab(&moved))
        .build_layered(k, layer);
    // The toolbar sizes its leading side to its content, and the strip sizes its tabs to its own
    // width: a basis wider than any window that gives way takes exactly the room the trailing
    // controls leave, decided by the layout and not by the tabs.
    strip.w(10_000.0).shrink().min_w(0.0)
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
    fn picking_sessions_opens_their_tabs_after_the_active_one() {
        let mut app = app();
        let (a, b) = (app.sessions[0].id, app.sessions[1].id);
        assert_eq!(app.tabs, [a], "the selected session's tab");
        app.select(b);
        app.select(a);
        let c = app.new_session();
        assert_eq!(app.tabs, [a, c, b], "a new one opens beside the active tab");
        app.select(b);
        assert_eq!(app.tabs.len(), 3, "an open tab isn't opened twice");
    }

    #[test]
    fn closing_the_active_tab_selects_its_neighbour_and_the_last_one_stays() {
        let mut app = app();
        let (a, b) = (app.sessions[0].id, app.sessions[1].id);
        app.select(b);
        app.close_tab(b);
        assert_eq!((app.tabs.as_slice(), app.selected), ([a].as_slice(), a));
        assert!(app.session(b).is_some(), "the session stays in the sidebar");
        app.close_tab(a);
        assert_eq!(app.tabs, [a], "the last tab stays");
    }

    #[test]
    fn deleting_a_session_closes_its_tab() {
        let mut app = app();
        let (a, b) = (app.sessions[0].id, app.sessions[1].id);
        app.select(b);
        app.ask_delete(b);
        app.confirm_delete();
        assert_eq!(app.tabs, [a]);
    }
}
