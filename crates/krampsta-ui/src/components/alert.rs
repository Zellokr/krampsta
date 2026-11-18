//! An alert in the page: a tinted card with a status icon, a title, an optional description
//! and an optional action. For what the user should read now (a run that failed, a sign-in that
//! didn't go through); a one-line hint stays a line of text.

use rune::{El, Icon, col, icon, row, text};
use rune_kit::{Kit, StateColors};

use crate::components::icons;

/// What the alert is about; picks its colours and icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Success,
    Warning,
    Danger,
}

impl Tone {
    fn colors(self, k: &Kit) -> StateColors {
        match self {
            Tone::Info => k.c.info,
            Tone::Success => k.c.success,
            Tone::Warning => k.c.warning,
            Tone::Danger => k.c.danger,
        }
    }

    fn icon(self) -> &'static Icon {
        match self {
            Tone::Info => &icons::INFO,
            Tone::Success => &icons::CIRCLE_CHECK,
            Tone::Warning => &icons::WARNING,
            Tone::Danger => &icons::CIRCLE_X,
        }
    }
}

pub struct Alert {
    tone: Tone,
    title: String,
    description: Option<String>,
    action: Option<El>,
}

impl Alert {
    pub fn new(tone: Tone, title: impl Into<String>) -> Alert {
        Alert { tone, title: title.into(), description: None, action: None }
    }

    /// The details under the title; selectable, so an error can be copied.
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }

    /// A control under the text, such as "Try again".
    pub fn action(mut self, a: El) -> Self {
        self.action = Some(a);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let c = self.tone.colors(k);
        let mut body = col()
            .gap(4.0)
            .shrink()
            .min_w(0.0)
            .child(text(self.title.clone()).font(k.ty.ui_medium).color(k.c.text_base).wrap());
        if let Some(d) = self.description {
            body = body.child(
                text(d).font(k.ty.small).color(k.c.text_muted).wrap().selectable(true).selection_color(k.selection()),
            );
        }
        if let Some(a) = self.action {
            body = body.child(row().pt(6.0).child(a));
        }
        row()
            .role("alert")
            .label(self.title)
            .w_full()
            .gap(10.0)
            .px(14.0)
            .py(12.0)
            .radius(k.radius.md)
            .fill(c.bg)
            .border(1.0, c.border)
            .child(icon(self.tone.icon(), 16.0).color(c.icon).mt(1.0))
            .child(body)
    }
}
