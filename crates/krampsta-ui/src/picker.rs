//! Choosing the provider and model: what the pickers show and what choosing does.
//!
//! The catalog says which providers exist; their model lists are fetched off the UI thread
//! (the host's runtime) and land through [`KrampstaApp::pump`]. Choosing rebuilds the engine for
//! runs started afterwards; a run already going keeps the engine it started with.

use std::collections::HashMap;

use krampsta_agent::{
    AgentError, Effort, EffortLevel, MOCK_MODEL, MOCK_PROVIDER, ModelList, Provider, ProviderKind, Selection,
};

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
    ComposerEffort,
    SettingsProvider,
    SettingsModel,
    SettingsEffort,
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
    /// How hard new runs ask the model to think; `None` leaves the model's default. Only sent
    /// to providers that take it.
    pub effort: Option<Effort>,
    /// The efforts the selected model takes, as its provider announced them; empty when it
    /// said nothing (see [`EngineChoice::effort_levels`]).
    pub efforts: Vec<EffortLevel>,
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
            effort: None,
            efforts: Vec::new(),
        }
    }

    /// The model last used with each provider.
    pub fn remembered_models(&self) -> &HashMap<String, String> {
        &self.remembered
    }

    /// Adds models used with providers before (from saved settings). Ones known already win.
    pub fn remember_models<'a>(&mut self, models: impl Iterator<Item = (&'a String, &'a String)>) {
        for (provider, model) in models {
            self.remembered.entry(provider.clone()).or_insert_with(|| model.clone());
        }
    }

    pub fn provider(&self) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == self.selection.provider)
    }

    /// The providers that can run now, the only ones the pickers list: an account provider once
    /// signed in, a cloud one once it has its key. Local servers always count: whether one is up
    /// shows when its models load.
    pub fn usable(&self) -> impl Iterator<Item = &Provider> {
        self.providers.iter().filter(|p| p.missing.is_none())
    }

    /// The usable providers under their group's header, groups in [`group`] order and providers
    /// in catalog order within each. The simulated agent only shows when nothing real can run,
    /// or while it is the one selected.
    pub fn grouped(&self) -> Vec<(&'static str, &Provider)> {
        let real = self.usable().any(|p| p.kind != ProviderKind::Simulated);
        let shown = |p: &&Provider| p.kind != ProviderKind::Simulated || !real || p.id == self.selection.provider;
        let mut list: Vec<_> = self.usable().filter(shown).map(|p| (group(&p.kind), p)).collect();
        list.sort_by_key(|((rank, _), _)| *rank);
        list.into_iter().map(|((_, header), p)| (header, p)).collect()
    }

    /// Whether the selected provider lets runs choose a reasoning effort.
    pub fn takes_effort(&self) -> bool {
        self.provider().is_some_and(|p| p.reasoning_effort)
    }

    /// Whether the selected provider reads the images a message carries.
    pub fn takes_images(&self) -> bool {
        self.provider().is_some_and(|p| p.images)
    }

    /// The efforts to offer: the model's own, or low/medium/high while it has said none.
    pub fn effort_levels(&self) -> Vec<EffortLevel> {
        if self.efforts.is_empty() { EffortLevel::defaults() } else { self.efforts.clone() }
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

    /// Switches to a usable provider (by id), restoring the model last used with it or its default.
    pub fn choose_provider(&mut self, id: &str) {
        self.engine.open = None;
        if id == self.engine.selection.provider || !self.engine.usable().any(|p| p.id == id) {
            return;
        }
        self.select_provider(id);
        self.rebuild_engine();
        if !matches!(self.engine.current_models(), Some(Models::Ready(_) | Models::Loading)) {
            self.refresh_models();
        }
    }

    /// Moves the selection off a provider that can't run (signed out, no key) to the first one
    /// that can, the simulated agent last. True when it moved; the caller rebuilds the engine.
    pub(crate) fn keep_selection_usable(&mut self) -> bool {
        if self.engine.usable().any(|p| p.id == self.engine.selection.provider) {
            return false;
        }
        let next = self.engine.usable().find(|p| p.id != MOCK_PROVIDER).or_else(|| self.engine.usable().next());
        let Some(id) = next.map(|p| p.id.clone()) else { return false };
        self.select_provider(&id);
        true
    }

    /// Points the selection at `id` with the model last used with it, its default or its first.
    fn select_provider(&mut self, id: &str) {
        let Some(provider) = self.engine.providers.iter().find(|p| p.id == id) else { return };
        let listed_first = match self.engine.models.get(id) {
            Some(Models::Ready(list)) => list.first().cloned(),
            _ => None,
        };
        let model = self.engine.remembered.get(id).cloned().or_else(|| provider.default_model.clone()).or(listed_first);
        self.engine.selection = Selection { provider: provider.id.clone(), model };
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

    /// Sets the effort for new runs; `None` leaves each model's default.
    pub fn choose_effort(&mut self, effort: Option<Effort>) {
        self.engine.open = None;
        self.engine.effort = effort;
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
        self.refresh_efforts();
    }

    /// Rereads the efforts the selected model takes. A chosen effort it doesn't take falls back
    /// to its default ("Auto").
    fn refresh_efforts(&mut self) {
        let Selection { provider, model } = &self.engine.selection;
        self.engine.efforts = match model {
            Some(model) if self.engine.takes_effort() => self.catalog.efforts(provider, model),
            _ => Vec::new(),
        };
        let levels = self.engine.effort_levels();
        if self.engine.effort.as_ref().is_some_and(|e| !levels.iter().any(|l| l.effort == *e)) {
            self.engine.effort = None;
        }
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
            } else if provider == self.engine.selection.provider {
                // The list brought the catalog the efforts come from.
                self.refresh_efforts();
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

/// A provider kind's place in the picker and the header it shows under.
pub fn group(kind: &ProviderKind) -> (u8, &'static str) {
    match kind {
        ProviderKind::Account(_) => (0, "Accounts"),
        ProviderKind::Cloud => (1, "API keys"),
        ProviderKind::Local => (2, "Local"),
        ProviderKind::Simulated => (3, "Testing"),
    }
}

/// What settings says about the providers the pickers leave out for a missing key ("OpenAI
/// needs OPENAI_API_KEY."). Accounts have their own cards. `None` when none is left out.
pub fn hidden_hint(choice: &EngineChoice) -> Option<String> {
    let hidden: Vec<String> = choice
        .providers
        .iter()
        .filter(|p| p.account().is_none())
        .filter_map(|p| p.missing.as_ref().map(|why| format!("{} {why}.", p.label)))
        .collect();
    (!hidden.is_empty()).then(|| hidden.join(" "))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use krampsta_agent::{Account, AgentEngine, BoxFuture, EngineCatalog, MockAgent, MockCatalog, MockConfig};

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
            kind: ProviderKind::Local,
            reasoning_effort: false,
            images: false,
            api_key: None,
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
        /// "local" model "a" announces low and xhigh; "b" announces nothing.
        fn efforts(&self, provider: &str, model: &str) -> Vec<EffortLevel> {
            match (provider, model) {
                ("local", "a") => {
                    ["low", "xhigh"].map(|id| EffortLevel { effort: Effort::new(id), description: None }).into()
                }
                _ => Vec::new(),
            }
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
        app.set_draft("hello");
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
    fn a_provider_without_its_key_is_left_out_and_settings_says_why() {
        let mut app = app();
        let listed: Vec<&str> = app.engine.usable().map(|p| p.id.as_str()).collect();
        assert_eq!(listed, ["local", "down", MOCK_PROVIDER]);
        app.choose_provider("cloud");
        assert_eq!(app.engine.selection.provider, MOCK_PROVIDER, "not choosable");
        assert_eq!(hidden_hint(&app.engine).as_deref(), Some("CLOUD needs KEY."));
    }

    #[test]
    fn an_unusable_starting_provider_gives_way_to_a_usable_one() {
        struct StartsOnCloud;
        impl EngineCatalog for StartsOnCloud {
            fn providers(&self) -> Vec<Provider> {
                Fake.providers()
            }
            fn initial(&self) -> Selection {
                Selection { provider: "cloud".into(), model: Some("big".into()) }
            }
            fn engine(&self, provider: &str, model: &str) -> Result<Arc<dyn AgentEngine>, AgentError> {
                Fake.engine(provider, model)
            }
            fn models(&self, provider: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
                Fake.models(provider)
            }
        }
        let app = KrampstaApp::new(Arc::new(StartsOnCloud));
        assert_eq!(app.engine.selection.provider, "local", "the first usable one, not the simulated agent");
    }

    #[test]
    fn the_picker_groups_providers_by_kind_in_a_fixed_order() {
        let of = |id: &str, kind: ProviderKind| Provider { kind, ..provider(id, None) };
        let signed_out = Provider {
            missing: Some("needs signing in".into()),
            ..of("copilot", ProviderKind::Account(Account::SignedOut))
        };
        let choice = EngineChoice::new(
            vec![
                of("ollama", ProviderKind::Local),
                MockCatalog::provider(),
                of("openai", ProviderKind::Cloud),
                of("chatgpt", ProviderKind::Account(Account::SignedIn { user: "me".into() })),
                signed_out,
                of("lmstudio", ProviderKind::Local),
            ],
            MockCatalog::default().initial(),
        );
        let shown: Vec<(&str, &str)> = choice.grouped().into_iter().map(|(g, p)| (g, p.id.as_str())).collect();
        assert_eq!(
            shown,
            [
                ("Accounts", "chatgpt"),
                ("API keys", "openai"),
                ("Local", "ollama"),
                ("Local", "lmstudio"),
                ("Testing", MOCK_PROVIDER),
            ],
            "the simulated agent shows while it is selected"
        );

        let mut choice = choice;
        choice.selection = Selection { provider: "ollama".into(), model: None };
        let groups: Vec<&str> = choice.grouped().into_iter().map(|(g, _)| g).collect();
        assert!(!groups.contains(&"Testing"), "hidden once something real is selected: {groups:?}");

        choice.providers.retain(|p| p.kind == ProviderKind::Simulated);
        let ids: Vec<&str> = choice.grouped().into_iter().map(|(_, p)| p.id.as_str()).collect();
        assert_eq!(ids, [MOCK_PROVIDER], "the only one that can run always shows");
    }

    #[test]
    fn the_efforts_offered_are_the_models_own_and_unknown_ones_fall_back_to_auto() {
        let ids = |app: &KrampstaApp| -> Vec<String> {
            app.engine.effort_levels().iter().map(|l| l.effort.as_str().to_owned()).collect()
        };
        let mut app = app();
        app.engine.providers[0].reasoning_effort = true;
        app.choose_provider("local");
        wait_for_models(&mut app, "local");
        assert_eq!(app.engine.selection.model.as_deref(), Some("a"));
        assert_eq!(ids(&app), ["low", "xhigh"], "model a's own levels");

        app.choose_effort(Some(Effort::new("xhigh")));
        app.choose_model("b");
        assert_eq!(ids(&app), ["low", "medium", "high"], "b announces none: the defaults");
        assert_eq!(app.engine.effort, None, "xhigh isn't offered for b, so back to Auto");

        app.choose_provider(MOCK_PROVIDER);
        assert!(app.engine.efforts.is_empty(), "the simulated agent takes no effort");
    }
}
