//! The "Files" view of the right panel: the project's files as a tree (rune-kit's `Tree`), a
//! filter above it, and a mark on files with a proposed change. Opening a file (double click
//! or Enter) mentions it in the composer. What it does lives in `files.rs`.

use rune::{El, Tx, col, text};
use rune_kit::{IconButton, IconButtonVariant, Kit, TextInput, Tree, TreeItem};

use crate::components::empty::Empty;
use crate::components::icons;
use crate::files::Listing;
use crate::keys;
use crate::state::KrampstaApp;

/// The refresh button the header shows while this view is up.
pub fn refresh(k: &Kit) -> El {
    IconButton::new(&icons::REFRESH)
        .variant(IconButtonVariant::GhostMuted)
        .label("Refresh the files")
        .help("Refresh")
        .on_press(|s: &mut KrampstaApp| s.refresh_files())
        .build(k)
        .key(keys::FILES_REFRESH)
}

pub fn body(s: &KrampstaApp, k: &Kit) -> El {
    let tree = &s.files;
    let state = |title: &str, description: &str| {
        let empty = Empty::new(&icons::FOLDER, title).description(description).build(k);
        col().key(keys::FILES_EMPTY).w_full().grow(1.0).min_h(0.0).px(24.0).justify_center().child(empty)
    };
    match &tree.listing {
        Listing::Failed(why) => return state("The files can't be listed", why),
        Listing::Stale | Listing::Loading if tree.model.is_empty() => {
            return state("Listing the files…", "Folders and files, as .gitignore leaves them.");
        }
        _ if tree.model.is_empty() => {
            return state("No files", "The project folder holds no files Git doesn't ignore.");
        }
        _ => {}
    }
    let filter = TextInput::new(&tree.filter)
        .placeholder("Filter files")
        .label("Filter files")
        .leading_icon(k.glyph.search)
        .clear_button()
        .width(None)
        .on_edit(|s: &mut KrampstaApp| &mut s.files.filter)
        .on_change(|s: &mut KrampstaApp, _: &mut Tx| s.apply_file_filter())
        .build(k)
        .key(keys::FILES_FILTER);
    // Files with a change waiting for review. A handful at most: copied per frame.
    let proposed: Vec<String> = s.review.proposals().iter().map(|c| c.path.clone()).collect();
    let query = tree.filter.text.trim().to_lowercase();
    let accent = k.c.warning.icon;
    let files = Tree::new(|s: &KrampstaApp| &s.files.model, |s: &mut KrampstaApp| &mut s.files.model, |n| &n.name)
        .name("Project files")
        .row(move |n, r| {
            let item = TreeItem::file(n, r).highlight(query.clone());
            // A folder is marked too, so a change inside a closed one still shows.
            let marked = if n.dir {
                proposed.iter().any(|p| p.strip_prefix(n.path.as_str()).is_some_and(|rest| rest.starts_with('/')))
            } else {
                proposed.contains(&n.path)
            };
            if marked { item.dot(accent) } else { item }
        })
        .on_open(|s: &mut KrampstaApp, id, _: &mut Tx| s.open_file(id))
        .build(s, k)
        .key(keys::FILES_TREE)
        .grow(1.0)
        .min_h(0.0);
    let mut part = col().w_full().grow(1.0).min_h(0.0).padding(8.0).gap(8.0).child(filter).child(files);
    if tree.truncated {
        let note = "The project has more files than the tree lists.";
        part = part.child(text(note).font(k.ty.small).color(k.c.text_faint).px(8.0));
    }
    part
}
