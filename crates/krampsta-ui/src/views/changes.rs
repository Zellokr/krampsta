//! The right-hand panel: the changes the agent proposed, as a diff to review. Each file is
//! accepted or rejected whole from its header, each block from under it.

use rune::{El, col, el, row, text};
use rune_kit::diff::{Annotation, DiffView};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::changes::parse_block_id;
use crate::components::alert::{Alert, Tone};
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
        // One copy of the kit (plain colours and type styles), shared by both builders.
        let kh = std::rc::Rc::new(k.clone());
        let ka = kh.clone();
        DiffView::new(|s: &KrampstaApp| &s.diff, |s: &mut KrampstaApp| &mut s.diff)
            .header_actions(move |s: &KrampstaApp, file| file_actions(s, file, &kh))
            .annotation(move |s: &KrampstaApp, a: &Annotation| block_actions(s, a, &ka))
            .build(s, k)
    };
    let mut panel = col()
        .key(keys::CHANGES)
        .role("region")
        .label("Changes")
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(k.c.bg_base)
        .child(header)
        .child(el().w_full().h(1.0).fill(k.c.border_weak));
    if let Some(error) = &s.review.error {
        let alert = Alert::new(Tone::Danger, "The change was not written").description(error).build(k);
        panel = panel.child(col().key(keys::CHANGES_ERROR).w_full().padding(12.0).child(alert));
    }
    panel.child(body)
}

fn small(label: &str, variant: ButtonVariant, busy: bool) -> Button {
    Button::new(label).variant(variant).control_size(ControlSize::Small).disabled(busy)
}

/// Reject and Accept for a whole file, in its header.
fn file_actions(s: &KrampstaApp, file: usize, k: &Kit) -> El {
    let busy = s.review.is_writing(file);
    let reject = small("Reject", ButtonVariant::Ghost, busy)
        .on_press(move |s: &mut KrampstaApp| s.reject_file(file))
        .build(k)
        .key(keys::change_reject(file));
    let accept = small(if busy { "Writing…" } else { "Accept" }, ButtonVariant::Contrast, busy)
        .on_press(move |s: &mut KrampstaApp| s.accept_file(file))
        .build(k)
        .key(keys::change_accept(file));
    row().gap(6.0).items_center().child(reject).child(accept)
}

/// Reject and Accept for one block, under its last changed line.
fn block_actions(s: &KrampstaApp, a: &Annotation, k: &Kit) -> El {
    let Some((file, block)) = parse_block_id(&a.id) else { return el() };
    let busy = s.review.is_writing(file);
    let reject = small("Reject block", ButtonVariant::GhostMuted, busy)
        .on_press(move |s: &mut KrampstaApp| s.reject_block(file, block))
        .build(k)
        .key(keys::block_reject(file, block));
    let accept = small("Accept block", ButtonVariant::Outline, busy)
        .on_press(move |s: &mut KrampstaApp| s.accept_block(file, block))
        .build(k)
        .key(keys::block_accept(file, block));
    row().w_full().px(12.0).py(4.0).gap(6.0).justify_end().items_center().child(reject).child(accept)
}
