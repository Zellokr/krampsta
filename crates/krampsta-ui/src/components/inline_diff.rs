//! A change shown inside its tool card: the file, how many lines it adds and removes, and the
//! changed lines with a little context, numbered, as a read-only unified diff. The review with
//! accept and reject stays in the changes panel (rune-kit's `DiffView`); this is the glance the
//! conversation needs. Long changes show their first lines and say how many more there are.

use rune::{El, col, row, text};
use rune_kit::Kit;
use rune_kit::diff::{FileDiff, LineKind};

use crate::theme;

/// The most diff lines a card shows.
const MAX_LINES: usize = 16;

pub fn view(diff: &FileDiff, k: &Kit) -> El {
    let lines = diff.hunks.iter().flat_map(|h| &h.lines);
    let (added, removed) = lines.clone().fold((0, 0), |(a, r), l| match l.kind {
        LineKind::Added => (a + 1, r),
        LineKind::Removed => (a, r + 1),
        LineKind::Context => (a, r),
    });
    let path = diff.new_path.as_deref().or(diff.old_path.as_deref()).unwrap_or_default();
    let (add_bg, add_ink) = theme::diff_line(k, true);
    let (del_bg, del_ink) = theme::diff_line(k, false);
    let header = row()
        .gap(8.0)
        .items_center()
        .child(text(path.to_owned()).font(k.ty.small_medium).color(k.c.text_base).truncate().shrink().min_w(0.0))
        .child(text(format!("+{added}")).font(k.ty.small).color(add_ink))
        .child(text(format!("-{removed}")).font(k.ty.small).color(del_ink));

    let mut body = col().w_full();
    let mut shown = 0;
    for (i, hunk) in diff.hunks.iter().enumerate() {
        if shown >= MAX_LINES {
            break;
        }
        if i > 0 {
            body = body.child(text("⋯").font(k.ty.small).color(k.c.text_faint).px(8.0));
        }
        for line in hunk.lines.iter().take(MAX_LINES - shown) {
            let (fill, sign, ink) = match line.kind {
                LineKind::Added => (Some(add_bg), "+", add_ink),
                LineKind::Removed => (Some(del_bg), "-", del_ink),
                LineKind::Context => (None, " ", k.c.text_faint),
            };
            let number = line.new.or(line.old).map(|n| n.to_string()).unwrap_or_default();
            let mut row = row()
                .w_full()
                .px(8.0)
                .gap(8.0)
                .child(text(number).font(k.ty.mono).color(k.c.text_faint).w(36.0))
                .child(text(sign).font(k.ty.mono).color(ink).w(10.0))
                .child(
                    text(line.text.clone()).font(k.ty.mono).color(k.c.text_base).wrap_anywhere().shrink().min_w(0.0),
                );
            if let Some(fill) = fill {
                row = row.fill(fill);
            }
            body = body.child(row);
            shown += 1;
        }
    }
    let total: usize = diff.hunks.iter().map(|h| h.lines.len()).sum();
    if total > shown {
        let more = total - shown;
        let note = format!("{more} more line{} in the changes panel", if more == 1 { "" } else { "s" });
        body = body.child(text(note).font(k.ty.small).color(k.c.text_faint).px(8.0).pt(4.0));
    }
    col()
        .w_full()
        .gap(6.0)
        .child(header)
        // Rounded, and the line washes kept inside its corners.
        .child(col().w_full().py(4.0).radius(k.radius.sm).clip_rounded(k.radius.sm).fill(k.c.layer_02).child(body))
}
