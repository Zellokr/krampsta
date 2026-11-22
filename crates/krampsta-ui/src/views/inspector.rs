//! The right-hand panel: a header that switches between "Changes" (the diff to review,
//! `views/changes.rs`), "Git" (what differs from the last commit, `views/git.rs`) and "Files"
//! (the project's files, `views/files.rs`), and the view chosen.

use rune::{El, Tx, col, el, row};
use rune_kit::{Kit, SegmentedControl};

use crate::files::Inspector;
use crate::keys;
use crate::state::KrampstaApp;
use crate::views::{changes, files, git};

pub fn view(s: &KrampstaApp, k: &Kit) -> El {
    let index = match s.inspector {
        Inspector::Changes => 0,
        Inspector::Git => 1,
        Inspector::Files => 2,
    };
    let tabs = SegmentedControl::new(&["Changes", "Git", "Files"], Some(index))
        .width(Some(252.0))
        .on_pick(|s: &mut KrampstaApp, i, _: &mut Tx| {
            s.show_inspector(match i {
                1 => Inspector::Git,
                2 => Inspector::Files,
                _ => Inspector::Changes,
            });
        })
        .build(k)
        .key(keys::INSPECTOR_TABS);
    // What the view says about itself (counts), and what it offers (refresh).
    let (info, action) = match s.inspector {
        Inspector::Changes => (changes::counts(s, k), None),
        Inspector::Git => (git::counts(s, k), Some(git::refresh(k))),
        Inspector::Files => (None, Some(files::refresh(k))),
    };
    // In a narrow panel the counts give way (cut at the edge); the switch and the button don't.
    let info = info.map(|i| row().shrink().min_w(0.0).justify_end().clip_rounded(0.0).child(i));
    let header = row()
        .w_full()
        .h(44.0)
        .px(12.0)
        .gap(8.0)
        .items_center()
        .child(tabs)
        .child(row().grow(1.0))
        .children(info)
        .children(action);
    let body = match s.inspector {
        Inspector::Changes => changes::body(s, k),
        Inspector::Git => git::body(s, k),
        Inspector::Files => files::body(s, k),
    };
    col()
        .key(keys::CHANGES)
        .role("region")
        .label(match s.inspector {
            Inspector::Changes => "Changes",
            Inspector::Git => "Git",
            Inspector::Files => "Files",
        })
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(k.c.bg_base)
        .child(header)
        .child(el().w_full().h(1.0).fill(k.c.border_weak))
        .child(body)
}
