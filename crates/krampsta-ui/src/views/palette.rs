//! The command palette's dialog: a field, and under it the rows that match (a name, and a
//! shortcut or project on the right), the active one filled. What it does lives in `palette.rs`.

use rune::{El, Key, Tx, col, row, text};
use rune_kit::{Dialog, Kit, TextInput};

use crate::keys;
use crate::state::KrampstaApp;

pub fn dialog(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>) {
    let palette = &s.palette;
    if !palette.open {
        return;
    }
    let field = TextInput::new(&palette.query)
        .placeholder("Type a command or a session…")
        .label("Command or session")
        .leading_icon(k.glyph.search)
        .width(None)
        .on_edit(|s: &mut KrampstaApp| &mut s.palette.query)
        .on_change(|s: &mut KrampstaApp, _: &mut Tx| s.refresh_palette())
        .on_key(|s: &mut KrampstaApp, ev, tx: &mut Tx| match ev.key {
            Key::Up => {
                s.palette_move(-1);
                true
            }
            Key::Down => {
                s.palette_move(1);
                true
            }
            Key::Enter => {
                s.run_palette(s.palette.active, tx);
                true
            }
            Key::Escape => {
                s.close_palette();
                true
            }
            _ => false,
        })
        .build(k)
        .key(keys::PALETTE_INPUT);
    let rows = palette.entries.iter().enumerate().map(|(i, entry)| {
        let active = i == palette.active;
        let mut line = row()
            .key(keys::palette_item(i))
            .role("option")
            .label(entry.label.clone())
            .w_full()
            .px(10.0)
            .py(7.0)
            .gap(12.0)
            .items_center()
            .radius(k.radius.sm)
            .hover(k.c.hover)
            .on_press_tx(move |s: &mut KrampstaApp, tx: &mut Tx| s.run_palette(i, tx))
            .child(
                text(entry.label.clone()).font(k.ty.ui).color(k.c.text_base).truncate().grow(1.0).shrink().min_w(0.0),
            );
        if !entry.detail.is_empty() {
            line = line.child(text(entry.detail.clone()).font(k.ty.small).color(k.c.text_faint));
        }
        if active { line.fill(k.c.layer_02) } else { line }
    });
    let mut list = col().w_full().gap(2.0).children(rows);
    if palette.entries.is_empty() {
        list = list.child(
            text("Nothing matches").font(k.ty.small).color(k.c.text_faint).px(10.0).py(8.0).key(keys::PALETTE_EMPTY),
        );
    }
    let body = col().w_full().gap(8.0).px(12.0).pb(12.0).child(field).child(list);
    Dialog::new(keys::PALETTE, "Commands")
        .open(true)
        .fit()
        .width(560.0)
        .click_outside(true)
        .child(body)
        .on_close(|s: &mut KrampstaApp| s.close_palette())
        .build(k, layer);
}
