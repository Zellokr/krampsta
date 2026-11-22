//! The right-hand panel: a header that switches between "Changes" (the diff to review,
//! `views/changes.rs`) and "Files" (the project's files, `views/files.rs`), and the view chosen.

use rune::{El, Tx, col, el, row};
use rune_kit::{Kit, SegmentedControl};

use crate::files::Inspector;
use crate::keys;
use crate::state::KrampstaApp;
use crate::views::{changes, files};

pub fn view(s: &KrampstaApp, k: &Kit) -> El {
    let index = match s.inspector {
        Inspector::Changes => 0,
        Inspector::Files => 1,
    };
    let tabs = SegmentedControl::new(&["Changes", "Files"], Some(index))
        .width(Some(170.0))
        .on_pick(|s: &mut KrampstaApp, i, _: &mut Tx| {
            s.show_inspector(if i == 1 { Inspector::Files } else { Inspector::Changes });
        })
        .build(k)
        .key(keys::INSPECTOR_TABS);
    let trailing = match s.inspector {
        Inspector::Changes => changes::counts(s, k),
        Inspector::Files => Some(files::refresh(k)),
    };
    let header =
        row().w_full().h(44.0).px(12.0).gap(10.0).items_center().child(tabs).child(row().grow(1.0)).children(trailing);
    let body = match s.inspector {
        Inspector::Changes => changes::body(s, k),
        Inspector::Files => files::body(s, k),
    };
    col()
        .key(keys::CHANGES)
        .role("region")
        .label(match s.inspector {
            Inspector::Changes => "Changes",
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
