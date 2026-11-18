//! Settings: a full-window page in the same frame as the chat.

use rune::{Cx, El, Tx, col, row, text};
use rune_kit::shell::Page;
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, SegmentedControl, Switch};

use crate::keys;
use crate::state::{KruneApp, Screen};
use crate::theme::ThemeChoice;

const THEMES: [ThemeChoice; 3] = [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System];

pub fn view(s: &KruneApp, cx: &mut Cx<KruneApp>, k: &Kit) -> El {
    let theme_index = THEMES.iter().position(|t| *t == s.theme);
    let appearance = SegmentedControl::new(&["Dark", "Light", "System"], theme_index)
        .width(Some(220.0))
        .on_pick(|s: &mut KruneApp, i, _: &mut Tx| {
            if let Some(t) = THEMES.get(i) {
                s.theme = *t;
            }
        })
        .build(k)
        .key(keys::SETTING_DARK);
    let changes = Switch::new(s.changes.is_open())
        .on_toggle(|s: &mut KruneApp| s.toggle_changes())
        .build(k)
        .key(keys::SETTING_CHANGES);
    let engine = text(format!("{} (simulated, no network)", s.engine_name())).font(k.ty.ui).color(k.c.text_muted);
    let version = text(concat!("Krune ", env!("CARGO_PKG_VERSION"))).font(k.ty.ui).color(k.c.text_muted);

    let body = col().w_full().h_full().padding(28.0).items_center().child(
        col()
            .w_full()
            .max_w(620.0)
            .gap(12.0)
            .child(text("General").font(k.ty.small_medium).color(k.c.text_faint))
            .child(setting(k, "Appearance", "Krune uses the dark theme by default.", appearance))
            .child(setting(k, "Changes panel", "Show file changes beside the conversation.", changes))
            .child(setting(k, "Agent engine", "Real providers arrive in a later milestone.", engine))
            .child(setting(k, "About", "Native AI Coding Agent, built with Rust and Rune.", version)),
    );
    let back = Button::new("Back")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut KruneApp| s.screen = Screen::Chat)
        .build(k)
        .key(keys::SETTINGS_BACK);
    Page::new(body.key(keys::SETTINGS_PAGE))
        .title("Settings")
        .leading(back)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .build(k)
}

fn setting(k: &Kit, title: &str, description: &str, control: El) -> El {
    row()
        .w_full()
        .py(12.0)
        .px(16.0)
        .gap(16.0)
        .radius(k.radius.md)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_weak)
        .items_center()
        .justify_between()
        .child(
            col()
                .gap(2.0)
                .shrink()
                .min_w(0.0)
                .child(text(title.to_owned()).font(k.ty.ui_medium).color(k.c.text_base))
                .child(text(description.to_owned()).font(k.ty.small).color(k.c.text_muted).wrap()),
        )
        .child(control)
}
