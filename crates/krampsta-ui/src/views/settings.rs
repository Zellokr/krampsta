//! Settings: a full-window page in the same frame as the chat.

use krampsta_agent::{KeySource, Provider};
use rune::{Cx, El, Icon, ScrollSpec, TextEdit, Tx, col, icon, row, text};
use rune_kit::shell::Page;
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, SegmentedControl, Switch, TextInput};

use crate::accounts::{KeyState, signed_in_user};
use crate::components::engine_picker::{self, Place};
use crate::components::icons;
use crate::costs::{PriceField, parse_dollars};
use crate::keys;
use crate::picker::hidden_hint;
use crate::state::{KrampstaApp, Screen};
use crate::theme::ThemeChoice;

const THEMES: [ThemeChoice; 3] = [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System];

pub fn view(s: &KrampstaApp, cx: &mut Cx<KrampstaApp>, k: &Kit, layer: &mut Vec<El>) -> El {
    let theme_index = THEMES.iter().position(|t| *t == s.theme);
    let appearance = SegmentedControl::new(&["Dark", "Light", "System"], theme_index)
        .width(Some(220.0))
        .on_pick(|s: &mut KrampstaApp, i, _: &mut Tx| {
            if let Some(t) = THEMES.get(i) {
                s.theme = *t;
            }
        })
        .build(k)
        .key(keys::SETTING_DARK);
    let changes = Switch::new(s.changes.is_open())
        .on_toggle(|s: &mut KrampstaApp| s.toggle_changes())
        .build(k)
        .key(keys::SETTING_CHANGES)
        .label("Changes panel");
    let provider = engine_picker::provider(s, k, layer, Place::Settings);
    let model = engine_picker::model(s, k, layer, Place::Settings);
    let provider_hint = match hidden_hint(&s.engine) {
        Some(missing) => format!("Only providers ready to use are listed. {missing}"),
        None => "Only providers ready to use are listed; sign in below to add an account.".to_owned(),
    };
    let warning = s.engine.warning();
    let model_hint = warning.as_deref().unwrap_or("Used for new messages; a running reply keeps its model.");
    let version = text(concat!("Krampsta ", env!("CARGO_PKG_VERSION"))).font(k.ty.ui).color(k.c.text_muted);

    let mut column = col()
        .w_full()
        .max_w(620.0)
        .gap(12.0)
        .child(section(k, "General"))
        .child(setting(k, "Appearance", "Krampsta uses the dark theme by default.", appearance))
        .child(setting(k, "Changes panel", "Show file changes beside the conversation.", changes))
        .child(section(k, "Agent").mt(12.0))
        .child(setting(k, "Provider", &provider_hint, provider))
        .child(setting(k, "Model", model_hint, model));
    if s.engine.takes_effort() {
        let effort = engine_picker::effort(s, k, layer, Place::Settings);
        let hint = "How long the model thinks first. Auto uses each model's own default.";
        column = column.child(setting(k, "Reasoning effort", hint, effort));
    }
    if s.key_providers().next().is_some() {
        column = column.child(section(k, "API keys").mt(12.0)).children(s.key_providers().map(|p| api_key(s, k, p)));
    }
    if !s.price_fields.is_empty() {
        let note = "US dollars per million tokens, as your provider charges them. A session shows its cost \
                    beside its tokens once every model it used has a price.";
        column = column
            .child(section(k, "Prices").mt(12.0))
            .child(text(note).font(k.ty.small).color(k.c.text_muted).wrap())
            .children(s.price_fields.iter().enumerate().map(|(i, f)| price(s, k, i, f)));
    }
    if s.account_providers().next().is_some() {
        let cards = row().w_full().gap(CARD_GAP).flex_wrap().children(s.account_providers().map(|p| account(k, p)));
        column = column.child(section(k, "Accounts").mt(12.0)).child(cards);
    }
    column = column.child(setting(k, "About", "Native AI Coding Agent, built with Rust and Rune.", version));
    // Scrolls when the window is too short for every row.
    let body = col()
        .key(keys::SETTINGS_SCROLL)
        .w_full()
        .h_full()
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(col().w_full().padding(28.0).items_center().child(column));
    let back = Button::new("Back")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Regular)
        .on_press(|s: &mut KrampstaApp| s.screen = Screen::Chat)
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

fn section(k: &Kit, title: &str) -> El {
    text(title.to_owned()).font(k.ty.small_medium).color(k.c.text_faint)
}

const CARD_GAP: f32 = 12.0;
/// Two cards a row across the settings column (620 points wide).
const CARD_W: f32 = (620.0 - CARD_GAP) / 2.0;

/// The mark shown on an account's card.
fn account_icon(id: &str) -> &'static Icon {
    match id {
        "chatgpt" => &icons::OPENAI,
        "copilot" => &icons::GITHUB,
        _ => &icons::KRAMPSTA,
    }
}

/// A provider used through an account, as a card: its mark, who is signed in, and the button
/// to sign in or out.
fn account(k: &Kit, p: &Provider) -> El {
    let id = p.id.clone();
    let (status, button) = match signed_in_user(p) {
        Some(user) => (
            user.to_owned(),
            Button::new("Sign out")
                .variant(ButtonVariant::Neutral)
                .on_press(move |s: &mut KrampstaApp| s.sign_out(&id)),
        ),
        None => (
            "Not signed in".to_owned(),
            Button::new("Sign in")
                .variant(ButtonVariant::Contrast)
                .on_press(move |s: &mut KrampstaApp| s.start_sign_in(&id)),
        ),
    };
    let control = button.control_size(ControlSize::Regular).build(k).key(keys::account(&p.id));
    col()
        .w(CARD_W)
        .py(20.0)
        .px(16.0)
        .gap(6.0)
        .radius(k.radius.md)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_weak)
        .items_center()
        .child(icon(account_icon(&p.id), 36.0).color(k.c.text_base))
        .child(text(p.label.clone()).font(k.ty.ui_medium).color(k.c.text_base).mt(6.0))
        .child(text(status).font(k.ty.small).color(k.c.text_muted).truncate())
        .child(control.mt(8.0))
}

/// A provider's API key: where it comes from, and a masked field to save one (or a button to
/// remove the saved one).
fn api_key(s: &KrampstaApp, k: &Kit, p: &Provider) -> El {
    let id = p.id.as_str();
    let field = &s.api_key;
    let busy = matches!(&field.state, KeyState::Saving { provider } if provider == id);
    let description = match (&field.state, &p.api_key) {
        (KeyState::Failed { provider, error }, _) if provider == id => error.clone(),
        (_, Some(KeySource::Saved)) => "Saved in the system keychain.".to_owned(),
        (_, Some(KeySource::Environment(var))) => format!("Read from {var}. A key saved here takes its place."),
        _ => "Not set. A key saved here goes to the system keychain.".to_owned(),
    };
    let control = if p.api_key == Some(KeySource::Saved) {
        let remove_id = p.id.clone();
        Button::new("Remove")
            .variant(ButtonVariant::Neutral)
            .control_size(ControlSize::Regular)
            .disabled(busy)
            .on_press(move |s: &mut KrampstaApp| s.remove_api_key(&remove_id))
            .build(k)
            .key(keys::api_key_remove(id))
    } else {
        let (submit_id, save_id) = (p.id.clone(), p.id.clone());
        let input = TextInput::new(&field.draft)
            .secure()
            .placeholder("sk-…")
            .label(format!("{} API key", p.label))
            .width(Some(200.0))
            .disabled(busy)
            .on_edit(|s: &mut KrampstaApp| &mut s.api_key.draft)
            .on_submit(move |s: &mut KrampstaApp, _: &mut Tx| s.save_api_key(&submit_id))
            .build(k)
            .key(keys::api_key(id));
        let save = Button::new("Save")
            .variant(ButtonVariant::Contrast)
            .control_size(ControlSize::Regular)
            .disabled(busy || field.draft.text.trim().is_empty())
            .on_press(move |s: &mut KrampstaApp| s.save_api_key(&save_id))
            .build(k)
            .key(keys::api_key_save(id));
        row().gap(8.0).items_center().child(input).child(save)
    };
    setting(k, &p.label, &description, control)
}

/// A billed model's price: what a million tokens in and out cost. A field that isn't a number
/// shows as invalid and leaves the price as it was.
fn price(s: &KrampstaApp, k: &Kit, index: usize, f: &PriceField) -> El {
    let provider = s.engine.providers.iter().find(|p| p.id == f.provider).map_or(f.provider.as_str(), |p| &p.label);
    let field = |edit: &TextEdit, side: &'static str| {
        let output = side == "output";
        TextInput::new(edit)
            .placeholder("$0.00")
            .label(format!("{} {side} price per million tokens", f.model))
            .width(Some(84.0))
            .invalid(parse_dollars(&edit.text).is_err())
            .on_edit(move |s: &mut KrampstaApp| s.price_text(index, output))
            .on_change(move |s: &mut KrampstaApp, _: &mut Tx| s.edit_price(index))
            .build(k)
            .key(keys::price(&f.provider, &f.model, side))
    };
    let caption = |t: &'static str| text(t).font(k.ty.small).color(k.c.text_muted);
    let control = row()
        .gap(8.0)
        .items_center()
        .child(caption("In"))
        .child(field(&f.input, "input"))
        .child(caption("Out"))
        .child(field(&f.output, "output"));
    let description = match s.price(&f.provider, &f.model) {
        Some(_) => provider.to_owned(),
        None => format!("{provider} · no price yet"),
    };
    setting(k, &f.model, &description, control)
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
