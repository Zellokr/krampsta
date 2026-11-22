//! The "Git" view of the right panel: what differs from the repository's last commit, as a
//! read-only diff (rune-kit's `DiffView`), with the files only named (binary or too big) under it.
//! What it does lives in `git.rs`.

use rune::{El, col, row, text};
use rune_kit::diff::DiffView;
use rune_kit::{IconButton, IconButtonVariant, Kit};

use crate::components::empty::Empty;
use crate::components::icons;
use crate::files::Listing;
use crate::keys;
use crate::state::KrampstaApp;

/// What the header says beside the switch: how many files, lines added and removed.
pub fn counts(s: &KrampstaApp, k: &Kit) -> Option<El> {
    let files = s.repo.diff.files();
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

/// The button that reads the repository again.
pub fn refresh(k: &Kit) -> El {
    IconButton::new(&icons::REFRESH)
        .variant(IconButtonVariant::GhostMuted)
        .label("Read the repository again")
        .help("Refresh")
        .on_press(|s: &mut KrampstaApp| s.refresh_repo())
        .build(k)
        .key(keys::GIT_REFRESH)
}

pub fn body(s: &KrampstaApp, k: &Kit) -> El {
    let repo = &s.repo;
    let state = |title: &str, description: &str| {
        let empty = Empty::new(&icons::REVIEW, title).description(description).build(k);
        col().key(keys::GIT_EMPTY).w_full().grow(1.0).min_h(0.0).px(24.0).justify_center().child(empty)
    };
    let nothing = repo.diff.files().is_empty() && repo.skipped.is_empty();
    match &repo.listing {
        Listing::Failed(why) => return state("Git can't be read", why),
        Listing::Stale | Listing::Loading if nothing => {
            return state("Reading the repository…", "What differs from the last commit.");
        }
        _ if nothing => {
            return state("No uncommitted changes", "The folder holds what the last commit does.");
        }
        _ => {}
    }
    let mut part = col().w_full().grow(1.0).min_h(0.0);
    if !repo.diff.files().is_empty() {
        let diff = DiffView::new(|s: &KrampstaApp| &s.repo.diff, |s: &mut KrampstaApp| &mut s.repo.diff)
            .build(s, k)
            .key(keys::GIT_DIFF);
        part = part.child(diff);
    }
    if !repo.skipped.is_empty() || repo.truncated {
        let mut note = String::new();
        if !repo.skipped.is_empty() {
            note.push_str("Changed, not compared (binary or too big): ");
            note.push_str(&repo.skipped.join(", "));
            note.push('.');
        }
        if repo.truncated {
            if !note.is_empty() {
                note.push(' ');
            }
            note.push_str("More files changed than are shown.");
        }
        part = part.child(
            text(note)
                .font(k.ty.small)
                .color(k.c.text_muted)
                .wrap()
                .selectable(true)
                .padding(12.0)
                .key(keys::GIT_SKIPPED),
        );
    }
    part
}
