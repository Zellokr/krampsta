//! The provider, model and reasoning-effort pickers: rune-kit `Select`s over [`EngineChoice`], shown under
//! the composer (inline, opening upwards) and in settings (field look).

use krampsta_agent::Effort;
use rune::{El, Tx};
use rune_kit::{Kit, Placement, Select, SelectAppearance};

use crate::keys;
use crate::picker::{Models, Picker};
use crate::state::KrampstaApp;

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

    fn effort(self) -> (Picker, &'static str) {
        match self {
            Place::Composer => (Picker::ComposerEffort, keys::COMPOSER_EFFORT),
            Place::Settings => (Picker::SettingsEffort, keys::SETTINGS_EFFORT),
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
pub fn provider(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>, place: Place) -> El {
    let (picker, _) = place.pickers();
    let (key, _) = place.keys();
    let choice = &s.engine;
    // Only what can run now, under a header per kind: signing in and keys are settings' business.
    let options: Vec<(&str, &str)> = choice.grouped().into_iter().map(|(g, p)| (g, p.label.as_str())).collect();
    place
        .style(Select::grouped(key, &options))
        .label("Provider")
        .current(Some(choice.provider_label()))
        .open(choice.open == Some(picker))
        .on_toggle(move |s: &mut KrampstaApp| s.toggle_picker(picker))
        .on_close(|s: &mut KrampstaApp| s.close_picker())
        .on_choose(|s: &mut KrampstaApp, label: &str, _: &mut Tx| {
            let id = s.engine.usable().find(|p| p.label == label).map(|p| p.id.clone());
            if let Some(id) = id {
                s.choose_provider(&id);
            }
        })
        .build(k, layer)
}

/// What the effort picker shows for a choice ("Auto effort" leaves the model's default).
pub fn effort_text(effort: Option<&Effort>) -> String {
    format!("{} effort", effort.map_or_else(|| "Auto".to_owned(), Effort::label))
}

/// The reasoning effort picker, for providers that take one ([`EngineChoice::takes_effort`]):
/// "Auto" and the levels the model announces, each with what it's for.
///
/// [`EngineChoice::takes_effort`]: crate::picker::EngineChoice::takes_effort
pub fn effort(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>, place: Place) -> El {
    let (picker, key) = place.effort();
    let levels = s.engine.effort_levels();
    let choices: Vec<Option<Effort>> =
        std::iter::once(None).chain(levels.iter().map(|l| Some(l.effort.clone()))).collect();
    let labels: Vec<String> = choices.iter().map(|e| effort_text(e.as_ref())).collect();
    let details: Vec<Option<String>> =
        std::iter::once(Some("Model default".to_owned())).chain(levels.into_iter().map(|l| l.description)).collect();
    place
        .style(Select::new(key, &labels))
        .details(&details)
        .label("Reasoning effort")
        .current(Some(&effort_text(s.engine.effort.as_ref())))
        .open(s.engine.open == Some(picker))
        .on_toggle(move |s: &mut KrampstaApp| s.toggle_picker(picker))
        .on_close(|s: &mut KrampstaApp| s.close_picker())
        .on_choose(move |s: &mut KrampstaApp, label: &str, _: &mut Tx| {
            if let Some(effort) = choices.iter().find(|e| effort_text(e.as_ref()) == label) {
                s.choose_effort(effort.clone());
            }
        })
        .build(k, layer)
}

/// The model picker for the selected provider.
pub fn model(s: &KrampstaApp, k: &Kit, layer: &mut Vec<El>, place: Place) -> El {
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
        .on_toggle(move |s: &mut KrampstaApp| s.toggle_picker(picker))
        .on_close(|s: &mut KrampstaApp| s.close_picker())
        .on_choose(|s: &mut KrampstaApp, model: &str, _: &mut Tx| s.choose_model(model))
        .build(k, layer)
}
