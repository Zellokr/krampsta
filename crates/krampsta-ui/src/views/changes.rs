//! The "Changes" view of the right panel: the changes the agent proposed, as a diff to review.
//! Each file is accepted or rejected whole from its header, each block from under it. The panel
//! itself (its header, and the switch to "Files") is `views/inspector.rs`.

use rune::{El, col, el, row, text};
use rune_kit::diff::{Annotation, DiffView};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::changes::parse_block_id;
use crate::components::alert::{Alert, Tone};
use crate::components::empty::Empty;
use crate::components::icons;
use crate::keys;
use crate::state::KrampstaApp;

/// What the header shows beside the view's name: how many files, lines added and removed.
pub fn counts(s: &KrampstaApp, k: &Kit) -> Option<El> {
    let files = s.diff.files();
    // Counts only say something once there is a change.
    if files.is_empty() {
        return None;
    }
    let (adds, dels) = files.iter().fold((0, 0), |(a, d), f| (a + f.additions(), d + f.deletions()));
    let count = match files.len() {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    Some(
        row()
            .gap(10.0)
            .items_center()
            .child(text(count).font(k.ty.small).color(k.c.text_muted))
            .child(text(format!("+{adds}")).font(k.ty.small_medium).color(k.c.success.icon))
            .child(text(format!("−{dels}")).font(k.ty.small_medium).color(k.c.danger.icon)),
    )
}

/// The diff to review (or the empty state), under a write error if the last one failed.
pub fn body(s: &KrampstaApp, k: &Kit) -> El {
    let files = s.diff.files();
    let content = if files.is_empty() {
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
    let mut part = col().w_full().grow(1.0).min_h(0.0);
    if let Some(error) = &s.review.error {
        let alert = Alert::new(Tone::Danger, "The change was not written").description(error).build(k);
        part = part.child(col().key(keys::CHANGES_ERROR).w_full().padding(12.0).child(alert));
    }
    part.child(content)
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
