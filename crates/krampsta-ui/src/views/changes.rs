//! The right-hand panel: file changes as a diff (sample data for now).

use rune::{El, col, el, row, text};
use rune_kit::Kit;
use rune_kit::diff::DiffView;

use crate::components::empty::Empty;
use crate::components::icons;
use crate::keys;
use crate::state::KrampstaApp;

pub fn view(s: &KrampstaApp, k: &Kit) -> El {
    let files = s.diff.files();
    let (adds, dels) = files.iter().fold((0, 0), |(a, d), f| (a + f.additions(), d + f.deletions()));
    let count = match files.len() {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    let mut header = row()
        .w_full()
        .h(44.0)
        .px(16.0)
        .gap(10.0)
        .items_center()
        .child(text("Changes").font(k.ty.ui_medium).color(k.c.text_base));
    // Counts only say something once there is a change.
    if !files.is_empty() {
        header = header
            .child(text(count).font(k.ty.small).color(k.c.text_muted))
            .child(row().grow(1.0))
            .child(text(format!("+{adds}")).font(k.ty.small_medium).color(k.c.success.icon))
            .child(text(format!("−{dels}")).font(k.ty.small_medium).color(k.c.danger.icon));
    }
    let body = if files.is_empty() {
        let empty = Empty::new(&icons::REVIEW, "No changes yet")
            .description("Edits Krampsta proposes show up here for you to review.")
            .build(k);
        col().key(keys::CHANGES_EMPTY).w_full().grow(1.0).min_h(0.0).px(24.0).justify_center().child(empty)
    } else {
        DiffView::new(|s: &KrampstaApp| &s.diff, |s: &mut KrampstaApp| &mut s.diff).build(s, k)
    };
    col()
        .key(keys::CHANGES)
        .role("region")
        .label("Changes")
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(k.c.bg_base)
        .child(header)
        .child(el().w_full().h(1.0).fill(k.c.border_weak))
        .child(body)
}
