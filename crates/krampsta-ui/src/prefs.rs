//! What the app remembers between launches: the theme, the side panes' widths, the provider
//! and model (and the model last used with each provider), the project folder and the project
//! groups folded away in the sidebar.
//!
//! The app reads them before its first frame (`krampsta-app`, one small file), so the window
//! opens as it was left. Saving needs no call at each place that changes one: `poll` compares
//! the state with what was saved last (no allocation) and, half a second after a change, hands
//! the new settings to the store, so dragging a pane doesn't write on every frame.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use krampsta_agent::Selection;
use rune::Tx;
use serde::{Deserialize, Serialize};

use crate::costs::Price;
use crate::state::KrampstaApp;
use crate::theme::ThemeChoice;

/// How long settings must stay unchanged before they are saved, in seconds.
const SETTLE: f64 = 0.5;

/// The saved settings. Every field is optional on disk: a missing or unknown one is ignored,
/// so files from older and newer builds both load.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: Option<ThemeChoice>,
    pub sidebar_width: Option<f32>,
    pub changes_width: Option<f32>,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// The model last used with each provider.
    pub models: BTreeMap<String, String>,
    pub project: Option<PathBuf>,
    /// Folded project groups (null: "No project").
    pub collapsed: Vec<Option<PathBuf>>,
    /// What billed models cost.
    pub prices: Vec<SavedPrice>,
}

/// A model's price as the settings file keeps it (dollars per million tokens).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPrice {
    pub provider: String,
    pub model: String,
    pub input: f64,
    pub output: f64,
}

impl Prefs {
    /// Reads them from what the store loaded. Err: a reason a person can read.
    pub fn from_json(value: serde_json::Value) -> Result<Prefs, String> {
        serde_json::from_value(value).map_err(|e| format!("the saved settings can't be read: {e}"))
    }
}

/// What was saved last, and since when the state has differed from it.
#[derive(Debug, Default)]
pub(crate) struct PrefsSync {
    saved: Prefs,
    changed_at: Option<f64>,
    /// The panes' widths at the last check: while they keep moving, the wait starts over.
    seen_widths: (f32, f32),
}

impl PrefsSync {
    /// Takes `prefs` as saved already (the state the app starts in).
    pub(crate) fn starting_at(prefs: Prefs) -> Self {
        let seen_widths = (prefs.sidebar_width.unwrap_or(0.0), prefs.changes_width.unwrap_or(0.0));
        PrefsSync { saved: prefs, changed_at: None, seen_widths }
    }
}

impl KrampstaApp {
    /// Opens as `prefs` left it. `engine`: also select their provider and model (false when the
    /// environment chose them, so `KRAMPSTA_PROVIDER` keeps working).
    pub fn with_prefs(mut self, prefs: Prefs, engine: bool) -> Self {
        if let Some(theme) = prefs.theme {
            self.theme = theme;
        }
        if let Some(w) = prefs.sidebar_width {
            self.sidebar.set_width(w);
        }
        if let Some(w) = prefs.changes_width {
            self.changes.set_width(w);
        }
        self.engine.remember_models(prefs.models.iter());
        if engine
            && let Some(provider) = prefs.provider.as_deref()
            && self.engine.usable().any(|p| p.id == provider)
        {
            self.engine.selection = Selection { provider: provider.to_owned(), model: prefs.model.clone() };
            self.rebuild_engine();
        }
        self.collapsed_groups = prefs.collapsed.iter().cloned().collect();
        self.prices = prefs
            .prices
            .iter()
            .map(|p| ((p.provider.clone(), p.model.clone()), Price { input: p.input, output: p.output }))
            .collect();
        // Only what changes after this is new; what couldn't apply isn't saved over.
        self.prefs = PrefsSync::starting_at(self.prefs_now());
        self
    }

    /// The settings as the app stands now.
    pub fn prefs_now(&self) -> Prefs {
        let Selection { provider, model } = &self.engine.selection;
        let mut collapsed: Vec<Option<PathBuf>> = self.collapsed_groups.iter().cloned().collect();
        collapsed.sort();
        Prefs {
            theme: Some(self.theme),
            sidebar_width: Some(self.sidebar.width()),
            changes_width: Some(self.changes.width()),
            provider: Some(provider.clone()),
            model: model.clone(),
            models: self.engine.remembered_models().iter().map(|(p, m)| (p.clone(), m.clone())).collect(),
            project: self.project.as_ref().map(|p| p.root.clone()),
            collapsed,
            prices: self
                .prices
                .iter()
                .map(|((provider, model), price)| SavedPrice {
                    provider: provider.clone(),
                    model: model.clone(),
                    input: price.input,
                    output: price.output,
                })
                .collect(),
        }
    }

    /// Whether the app stands as `p` says, without building anything (runs every frame).
    fn prefs_match(&self, p: &Prefs) -> bool {
        let selection = &self.engine.selection;
        let remembered = self.engine.remembered_models();
        p.theme == Some(self.theme)
            && p.sidebar_width == Some(self.sidebar.width())
            && p.changes_width == Some(self.changes.width())
            && p.provider.as_deref() == Some(selection.provider.as_str())
            && p.model == selection.model
            && p.models.len() == remembered.len()
            && remembered.iter().all(|(provider, model)| p.models.get(provider) == Some(model))
            && p.project.as_deref() == self.project.as_ref().map(|x| x.root.as_path())
            && p.collapsed.len() == self.collapsed_groups.len()
            && p.collapsed.iter().all(|g| self.collapsed_groups.contains(g))
            // `saved` always comes from `prefs_now`, in the map's (sorted) order.
            && p.prices.len() == self.prices.len()
            && p.prices.iter().zip(&self.prices).all(|(saved, ((provider, model), price))| {
                saved.provider == *provider
                    && saved.model == *model
                    && saved.input == price.input
                    && saved.output == price.output
            })
    }

    /// Saves the settings once they have settled after a change (`poll` calls it with its clock).
    pub fn sync_prefs(&mut self, clock: f64, tx: &mut Tx) {
        let Some(store) = self.store.clone() else { return };
        if self.prefs_match(&self.prefs.saved) {
            self.prefs.changed_at = None;
            return;
        }
        let widths = (self.sidebar.width(), self.changes.width());
        if widths != self.prefs.seen_widths || self.sidebar.is_resizing() || self.changes.is_resizing() {
            self.prefs.seen_widths = widths;
            self.prefs.changed_at = Some(clock);
        }
        let since = *self.prefs.changed_at.get_or_insert(clock);
        let wait = SETTLE - (clock - since);
        if wait > 0.0 {
            tx.wake_after(Duration::from_secs_f64(wait));
            return;
        }
        let now = self.prefs_now();
        match serde_json::to_value(&now) {
            Ok(value) => store.save_settings(value),
            Err(e) => self.notice = Some(format!("the settings can't be saved: {e}")),
        }
        self.prefs.saved = now;
        self.prefs.changed_at = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use krampsta_agent::{MemoryStore, MockCatalog, MockConfig};

    use super::*;

    #[test]
    fn settings_from_older_or_newer_builds_still_load() {
        let prefs = Prefs::from_json(serde_json::json!({ "theme": "light", "someday": 1 })).unwrap();
        assert_eq!(prefs.theme, Some(ThemeChoice::Light));
        assert_eq!(prefs.provider, None);
        assert!(Prefs::from_json(serde_json::json!({ "theme": 3 })).is_err());
    }

    #[test]
    fn opening_with_settings_applies_them_and_saves_nothing_new() {
        let store = Arc::new(MemoryStore::new());
        let prefs = Prefs {
            theme: Some(ThemeChoice::Light),
            sidebar_width: Some(300.0),
            changes_width: Some(500.0),
            collapsed: vec![Some("/code/rune".into()), None],
            ..Prefs::default()
        };
        let app = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant())))
            .with_store(store.clone())
            .with_prefs(prefs, true);
        assert_eq!(app.theme, ThemeChoice::Light);
        assert_eq!(app.sidebar.width(), 300.0);
        assert_eq!(app.changes.width(), 500.0);
        assert!(app.is_group_collapsed(None));
        assert!(app.prefs_match(&app.prefs.saved), "nothing to save right after opening");
    }

    #[test]
    fn an_unknown_provider_keeps_the_catalogs_choice() {
        let prefs = Prefs { provider: Some("gone".into()), model: Some("x".into()), ..Prefs::default() };
        let app = KrampstaApp::new(Arc::new(MockCatalog::new(MockConfig::instant()))).with_prefs(prefs, true);
        assert_ne!(app.engine.selection.provider, "gone");
    }
}
