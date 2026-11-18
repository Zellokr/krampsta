//! An empty state: what a place shows before it has anything (no messages, no changes), with
//! a mark, a line saying what goes here and how to fill it, and an optional action.

use rune::{Align, Color, El, Icon, col, icon, row, text};
use rune_kit::Kit;

pub struct Empty {
    mark: &'static Icon,
    mark_size: f32,
    mark_color: Option<Color>,
    title: String,
    description: Option<String>,
    action: Option<El>,
}

impl Empty {
    pub fn new(mark: &'static Icon, title: impl Into<String>) -> Empty {
        Empty { mark, mark_size: 24.0, mark_color: None, title: title.into(), description: None, action: None }
    }

    /// A bigger mark, for a whole pane (the chat) rather than a panel.
    pub fn large(mut self) -> Self {
        self.mark_size = 36.0;
        self
    }

    /// The mark's colour; muted by default.
    pub fn mark_color(mut self, c: Color) -> Self {
        self.mark_color = Some(c);
        self
    }

    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }

    pub fn action(mut self, a: El) -> Self {
        self.action = Some(a);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let title_font = if self.mark_size > 24.0 { k.ty.body_medium } else { k.ty.ui_medium };
        let mut out = col()
            .role("status")
            .label(self.title.clone())
            .w_full()
            .gap(12.0)
            .items_center()
            .child(icon(self.mark, self.mark_size).color(self.mark_color.unwrap_or(k.c.icon_weaker)))
            .child(text(self.title).font(title_font).color(k.c.text_base).wrap().align(Align::Center));
        if let Some(d) = self.description {
            out = out.child(text(d).font(k.ty.ui).color(k.c.text_muted).wrap().align(Align::Center));
        }
        if let Some(a) = self.action {
            out = out.child(row().pt(4.0).child(a));
        }
        out
    }
}
