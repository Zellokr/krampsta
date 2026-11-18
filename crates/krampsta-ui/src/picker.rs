//! Choosing the provider and model: what the pickers show and what choosing does.
//!
//! The catalog says which providers exist; their model lists are fetched off the UI thread
//! (the host's runtime) and land through [`KrampstaApp::pump`]. Choosing rebuilds the engine for
//! runs started afterwards; a run already going keeps the engine it started with.

use std::collections::HashMap;

use krampsta_agent::{Account, AgentError, MOCK_MODEL, MOCK_PROVIDER, ModelList, Provider, Selection};

use crate::state::KrampstaApp;

/// A provider's model list, as far as we know it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Models {
    Loading,
    Ready(Vec<String>),
    Failed(String),
}

/// Which picker's listbox is open (each place has its own pair).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picker {
    ComposerProvider,
    ComposerModel,
    SettingsProvider,
    SettingsModel,
}

impl Picker {
    pub fn is_model(self) -> bool {
        matches!(self, Picker::ComposerModel | Picker::SettingsModel)
    }
}

/// The picker state [`KrampstaApp`] keeps.
#[derive(Debug)]
pub struct EngineChoice {
    pub providers: Vec<Provider>,
    pub selection: Selection,
    pub models: HashMap<String, Models>,
    /// The model last chosen per provider, so switching back restores it.
    remembered: HashMap<String, String>,
    pub open: Option<Picker>,
    /// Why the selection can't run (no model yet, missing key…). `None` when it can.
    pub problem: Option<String>,
}

impl EngineChoice {
    pub fn new(providers: Vec<Provider>, selection: Selection) -> Self {
        EngineChoice {
            providers,
            selection,
            models: HashMap::new(),
            remembered: HashMap::new(),
            open: None,
            problem: None,
        }
    }

    pub fn provider(&self) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == self.selection.provider)
    }

    pub fn provider_label(&self) -> &str {
        self.provider().map_or(self.selection.provider.as_str(), |p| p.label.as_str())
    }

    /// The selected provider's model list.
    pub fn current_models(&self) -> Option<&Models> {
        self.models.get(&self.selection.provider)
    }

    /// What to tell the user about the selection, most useful first: its model list could not
    /// be read, or it can't run yet. `None` when all is well.
    pub fn warning(&self) -> Option<String> {
        match self.current_models() {
            Some(Models::Failed(why)) => Some(format!("{}: {why}", self.provider_label())),
            _ => self.problem.clone(),
        }
    }
}

impl KrampstaApp {
    /// Opens or closes a picker. Opening a model list that failed to load tries again.
    pub fn toggle_picker(&mut self, picker: Picker) {
        if self.engine.open == Some(picker) {
            self.engine.open = None;
            return;
        }
        self.engine.open = Some(picker);
        if picker.is_model() && !matches!(self.engine.current_models(), Some(Models::Ready(_) | Models::Loading)) {
            self.refresh_models();
        }
    }

    pub fn close_picker(&mut self) {
        self.engine.open = None;
    }

    /// Switches provider (by id), restoring the model last used with it or its default. An
    /// account provider that isn't signed in opens the sign-in instead of loading models.
    pub fn choose_provider(&mut self, id: &str) {
        self.engine.open = None;
        let Some(provider) = self.engine.providers.iter().find(|p| p.id == id).cloned() else { return };
        let signed_out = provider.account == Some(Account::SignedOut);
        if provider.id == self.engine.selection.provider {
            if signed_out {
                self.start_sign_in(id);
            }
            return;
        }
        let listed_first = match self.engine.models.get(id) {
            Some(Models::Ready(list)) => list.first().cloned(),
            _ => None,
        };
        let model = self.engine.remembered.get(id).cloned().or(provider.default_model).or(listed_first);
        self.engine.selection = Selection { provider: provider.id, model };
        self.rebuild_engine();
        if signed_out {
            self.start_sign_in(id);
        } else if !matches!(self.engine.current_models(), Some(Models::Ready(_) | Models::Loading)) {
            self.refresh_models();
        }
    }

    /// Picks a model of the selected provider. Only listed models (or the current one) count:
    /// the listbox's status rows ("Loading…") are not models.
    pub fn choose_model(&mut self, model: &str) {
        self.engine.open = None;
        let listed = matches!(self.engine.current_models(), Some(Models::Ready(l)) if l.iter().any(|m| m == model));
        if !listed {
            return;
        }
        self.engine.selection.model = Some(model.to_owned());
        self.rebuild_engine();
    }

    /// Loads the selected provider's models unless they are known or on their way.
    pub fn ensure_models(&mut self) {
        if self.engine.current_models().is_none() {
            self.refresh_models();
        }
    }

    /// Asks the catalog for the selected provider's models, in the background.
    pub fn refresh_models(&mut self) {
        let provider = self.engine.selection.provider.clone();
        if provider == MOCK_PROVIDER {
            // Known without asking anyone: no need to start the runtime for it.
            self.engine.models.insert(provider, Models::Ready(vec![MOCK_MODEL.to_owned()]));
            return;
        }
        let lookup = self.catalog.models(&provider);
        let state = match self.agent.fetch_models(&provider, lookup) {
            Ok(()) => Models::Loading,
            Err(e) => Models::Failed(e.to_string()),
        };
        self.engine.models.insert(provider, state);
    }

    /// Builds the engine for the selection; problems are kept for the pickers and the composer.
    pub(crate) fn rebuild_engine(&mut self) {
        let Selection { provider, model } = self.engine.selection.clone();
        self.engine.problem = match model {
            None => Some(format!("Choose a model for {}.", self.engine.provider_label())),
            Some(model) => match self.catalog.engine(&provider, &model) {
                Ok(engine) => {
                    self.agent.set_engine(engine);
                    self.engine.remembered.insert(provider, model);
                    None
                }
                Err(e) => Some(reason(&e)),
            },
        };
    }

    /// Stores model lists that arrived. A provider with no model yet takes its first one.
    pub(crate) fn land_models(&mut self, lists: Vec<ModelList>) -> bool {
        let changed = !lists.is_empty();
        for ModelList { provider, models } in lists {
            let state = match models {
                Ok(list) if list.is_empty() => Models::Failed("the server lists no models".into()),
                Ok(list) => Models::Ready(list),
                Err(e) => Models::Failed(reason(&e)),
            };
            let first = match &state {
                Models::Ready(list) => list.first().cloned(),
                _ => None,
            };
            self.engine.models.insert(provider.clone(), state);
            if provider == self.engine.selection.provider && self.engine.selection.model.is_none() && first.is_some() {
                self.engine.selection.model = first;
                self.rebuild_engine();
            }
        }
        changed
    }
}

/// An error's text without the "provider error:" prefix, for a line that already names the provider.
fn reason(e: &AgentError) -> String {
    match e {
        AgentError::Provider(message) => message.clone(),
        other => other.to_string(),
    }
}

/// What a picker shows for a provider: its name, marked "(no key)" when a key is missing (the
/// hint line says which once it is chosen). Account providers carry no mark: choosing one that
/// is signed out opens the sign-in.
pub fn provider_text(p: &Provider) -> String {
    match &p.missing {
        Some(_) if p.account.is_none() => format!("{} (no key)", p.label),
        _ => p.label.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use krampsta_agent::{AgentEngine, BoxFuture, EngineCatalog, MockAgent, MockCatalog, MockConfig};
    use rune::TextEdit;

    use super::*;
    use crate::state::SendError;

    /// A local server with two models, one whose server is down, one missing its key, and
    /// the simulated agent.
    struct Fake;

    fn provider(id: &str, missing: Option<&str>) -> Provider {
        Provider {
            id: id.into(),
            label: id.to_uppercase(),
            default_model: None,
            missing: missing.map(Into::into),
            account: None,
        }
    }

    impl EngineCatalog for Fake {
        fn providers(&self) -> Vec<Provider> {
            vec![
                provider("local", None),
                provider("down", None),
                provider("cloud", Some("needs KEY")),
                MockCatalog::provider(),
            ]
        }
        fn initial(&self) -> Selection {
            MockCatalog::default().initial()
        }
        fn engine(&self, provider: &str, model: &str) -> Result<Arc<dyn AgentEngine>, AgentError> {
            match provider {
                "cloud" => Err(AgentError::Provider("CLOUD needs KEY".into())),
                _ if model.is_empty() => Err(AgentError::Provider("no model".into())),
                _ => Ok(Arc::new(MockAgent::new(MockConfig::instant()))),
            }
        }
        fn models(&self, provider: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
            let answer = match provider {
                "local" => Ok(vec!["a".to_owned(), "b".to_owned()]),
                _ => Err(AgentError::Provider("could not reach the server".into())),
            };
            Box::pin(async move { answer })
        }
    }

    fn app() -> KrampstaApp {
        KrampstaApp::new(Arc::new(Fake))
    }

    /// Pumps until `provider`'s model list has landed.
    fn wait_for_models(app: &mut KrampstaApp, provider: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(app.engine.models.get(provider), Some(Models::Loading) | None) {
            assert!(Instant::now() < deadline, "the model list never arrived");
            app.pump();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn send(app: &mut KrampstaApp) -> Result<krampsta_core::RunId, SendError> {
        app.draft = TextEdit::new("hello");
        app.send()
    }

    #[test]
    fn the_simulated_agent_is_ready_at_once_without_a_lookup() {
        let mut app = app();
        app.ensure_models();
        assert_eq!(app.engine.current_models(), Some(&Models::Ready(vec![MOCK_MODEL.to_owned()])));
        assert_eq!(app.engine.warning(), None);
        assert!(send(&mut app).is_ok());
    }

    #[test]
    fn a_local_provider_loads_its_models_and_takes_the_first() {
        let mut app = app();
        app.choose_provider("local");
        assert_eq!(app.engine.current_models(), Some(&Models::Loading));
        assert_eq!(app.engine.warning().as_deref(), Some("Choose a model for LOCAL."));
        assert!(matches!(send(&mut app), Err(SendError::Engine(_))), "no model yet: nothing is sent");

        wait_for_models(&mut app, "local");
        assert_eq!(app.engine.selection.model.as_deref(), Some("a"));
        assert_eq!(app.engine.warning(), None);
        app.choose_model("b");
        assert_eq!(app.engine.selection.model.as_deref(), Some("b"));
        assert!(send(&mut app).is_ok());
    }

    #[test]
    fn status_rows_and_unknown_models_are_not_choosable() {
        let mut app = app();
        app.choose_provider("local");
        app.choose_model("Loading…");
        assert_eq!(app.engine.selection.model, None);
        wait_for_models(&mut app, "local");
        app.choose_model("zzz");
        assert_eq!(app.engine.selection.model.as_deref(), Some("a"));
    }

    #[test]
    fn switching_back_restores_the_model_used_last() {
        let mut app = app();
        app.choose_provider("local");
        wait_for_models(&mut app, "local");
        app.choose_model("b");
        app.choose_provider(MOCK_PROVIDER);
        assert_eq!(app.engine.selection.model.as_deref(), Some(MOCK_MODEL));
        app.choose_provider("local");
        assert_eq!(app.engine.selection.model.as_deref(), Some("b"));
    }

    #[test]
    fn an_unreachable_server_says_so_without_the_error_prefix() {
        let mut app = app();
        app.choose_provider("down");
        wait_for_models(&mut app, "down");
        assert_eq!(app.engine.warning().as_deref(), Some("DOWN: could not reach the server"));
        // Opening the list again retries.
        app.toggle_picker(Picker::ComposerModel);
        assert_eq!(app.engine.current_models(), Some(&Models::Loading));
    }

    #[test]
    fn a_provider_without_its_key_explains_and_refuses_to_send() {
        let mut app = app();
        assert_eq!(provider_text(&app.engine.providers[2]), "CLOUD (no key)");
        app.engine.providers[2].default_model = Some("big".into());
        app.choose_provider("cloud");
        assert_eq!(app.engine.problem.as_deref(), Some("CLOUD needs KEY"));
        assert!(matches!(send(&mut app), Err(SendError::Engine(_))));
    }
}
