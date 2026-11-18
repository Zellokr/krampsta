//! The provider and model pickers: two rune-kit `Select`s over [`EngineChoice`], shown under
//! the composer (inline, opening upwards) and in settings (field look).

use rune::{El, Tx};
use rune_kit::{Kit, Placement, Select, SelectAppearance};

use crate::keys;
use crate::picker::{Models, Picker, provider_text};
use crate::state::KruneApp;

/// Where a pair of pickers sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Composer,
    Settings,
}

impl Place {
    fn pickers(self) -> (Picker, Picker) {
        match self {
            Place::Composer => (Picker::ComposerProvider, Picker::ComposerModel),
            Place::Settings => (Picker::SettingsProvider, Picker::SettingsModel),
        }
    }

    fn keys(self) -> (&'static str, &'static str) {
        match self {
            Place::Composer => (keys::COMPOSER_PROVIDER, keys::COMPOSER_MODEL),
            Place::Settings => (keys::SETTINGS_PROVIDER, keys::SETTINGS_MODEL),
        }
    }

    fn style(self, select: Select) -> Select {
        match self {
            Place::Composer => select.appearance(SelectAppearance::Inline).placement(Placement::TopEnd),
            Place::Settings => select.appearance(SelectAppearance::Field).width(240.0).placement(Placement::BottomEnd),
        }
    }
}

/// Shown in the model list while it loads, and when it could not be read (the hint line says why).
pub const LOADING: &str = "Loading…";
pub const UNAVAILABLE: &str = "No models";

/// The provider picker.
pub fn provider(s: &KruneApp, k: &Kit, layer: &mut Vec<El>, place: Place) -> El {
    let (picker, _) = place.pickers();
    let (key, _) = place.keys();
    let choice = &s.engine;
    // Unusable providers are marked in their own row ("OpenAI (no key)").
    let labels: Vec<String> = choice.providers.iter().map(provider_text).collect();
    place
        .style(Select::new(key, &labels))
        .label("Provider")
        .current(choice.provider().map(provider_text).as_deref())
        .open(choice.open == Some(picker))
        .on_toggle(move |s: &mut KruneApp| s.toggle_picker(picker))
        .on_close(|s: &mut KruneApp| s.close_picker())
        .on_choose(|s: &mut KruneApp, label: &str, _: &mut Tx| {
            let id = s.engine.providers.iter().find(|p| provider_text(p) == label).map(|p| p.id.clone());
            if let Some(id) = id {
                s.choose_provider(&id);
            }
        })
        .build(k, layer)
}

/// The model picker for the selected provider.
pub fn model(s: &KruneApp, k: &Kit, layer: &mut Vec<El>, place: Place) -> El {
    let (_, picker) = place.pickers();
    let (_, key) = place.keys();
    let choice = &s.engine;
    let current = choice.selection.model.as_deref();
    let mut options: Vec<String> = match choice.current_models() {
        Some(Models::Ready(list)) => list.clone(),
        Some(Models::Loading) => vec![LOADING.into()],
        Some(Models::Failed(_)) => vec![UNAVAILABLE.into()],
        None => Vec::new(),
    };
    // The current model always shows, even before (or without) a list naming it.
    if let Some(m) = current.filter(|m| !options.iter().any(|o| o == m)) {
        options.insert(0, m.to_owned());
    }
    place
        .style(Select::new(key, &options))
        .label("Model")
        .placeholder("Choose a model")
        .current(current)
        .invalid(choice.warning().is_some())
        .open(choice.open == Some(picker))
        .on_toggle(move |s: &mut KruneApp| s.toggle_picker(picker))
        .on_close(|s: &mut KruneApp| s.close_picker())
        .on_choose(|s: &mut KruneApp, model: &str, _: &mut Tx| s.choose_model(model))
        .build(k, layer)
}
