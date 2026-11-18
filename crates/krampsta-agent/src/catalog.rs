//! The engine catalog: which providers exist, which models each offers, and how to build an
//! engine for a provider and model. The UI shows the catalog and switches engines through it,
//! without knowing any provider.

use std::sync::Arc;

use crate::engine::{AgentEngine, AgentError, BoxFuture};
use crate::mock::{MockAgent, MockConfig};

/// One provider the user can pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    /// Stable id ("openai", "ollama", "mock").
    pub id: String,
    /// Shown in pickers ("OpenAI", "Ollama").
    pub label: String,
    /// The model chosen when the user picks this provider; `None` until its list is known.
    pub default_model: Option<String>,
    /// Why it can't be used yet ("needs OPENAI_API_KEY"); `None` when it is ready.
    pub missing: Option<String>,
    /// For providers used through a personal account (GitHub Copilot, a ChatGPT plan): whether
    /// the user is signed in. `None` for providers without accounts.
    pub account: Option<Account>,
}

/// The sign-in state of a provider that works through an account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Account {
    SignedOut,
    /// `user` names the account ("octocat", "me@example.com").
    SignedIn {
        user: String,
    },
}

/// What a device sign-in asks the user to do: open `url` and type `code`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCode {
    pub url: String,
    pub code: String,
}

/// Called by a sign-in once it has a code for the user (from the host's runtime).
pub type LoginPrompt = Arc<dyn Fn(DeviceCode) + Send + Sync>;

/// A provider and one of its models.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub provider: String,
    pub model: Option<String>,
}

/// Everything the UI may run.
pub trait EngineCatalog: Send + Sync + 'static {
    /// The providers, in the order pickers list them.
    fn providers(&self) -> Vec<Provider>;

    /// What to select at start.
    fn initial(&self) -> Selection;

    /// An engine for `provider` running `model`. Cheap: no network.
    fn engine(&self, provider: &str, model: &str) -> Result<Arc<dyn AgentEngine>, AgentError>;

    /// The models `provider` offers right now (may ask the server). Runs on the host's runtime.
    fn models(&self, provider: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>>;

    /// Signs in to `provider`'s account with a device code: calls `prompt` once the code is
    /// known, then waits for the user to approve it, stores the credentials and resolves to the
    /// account's name. Runs on the host's runtime; the host cancels it by dropping it.
    fn login(&self, provider: &str, prompt: LoginPrompt) -> BoxFuture<'static, Result<String, AgentError>> {
        let _ = prompt;
        let error = AgentError::Provider(format!("`{provider}` has no account to sign in to"));
        Box::pin(async move { Err(error) })
    }

    /// Forgets `provider`'s stored credentials.
    fn logout(&self, provider: &str) -> BoxFuture<'static, Result<(), AgentError>> {
        let _ = provider;
        Box::pin(async { Ok(()) })
    }
}

/// The simulated agent's id and its one model.
pub const MOCK_PROVIDER: &str = "mock";
pub const MOCK_MODEL: &str = "simulated";

/// A catalog with only the simulated agent, for tests and when nothing else is configured.
#[derive(Debug, Default)]
pub struct MockCatalog {
    config: MockConfig,
}

impl MockCatalog {
    pub fn new(config: MockConfig) -> Self {
        MockCatalog { config }
    }

    /// The simulated provider, as other catalogs list it too.
    pub fn provider() -> Provider {
        Provider {
            id: MOCK_PROVIDER.into(),
            label: "Simulated".into(),
            default_model: Some(MOCK_MODEL.into()),
            missing: None,
            account: None,
        }
    }

    /// Its engine, for any catalog that offers it.
    pub fn engine_with(config: MockConfig, model: &str) -> Result<Arc<dyn AgentEngine>, AgentError> {
        if model == MOCK_MODEL {
            Ok(Arc::new(MockAgent::new(config)))
        } else {
            Err(AgentError::Provider(format!("the simulated agent has no model `{model}`")))
        }
    }
}

impl EngineCatalog for MockCatalog {
    fn providers(&self) -> Vec<Provider> {
        vec![Self::provider()]
    }

    fn initial(&self) -> Selection {
        Selection { provider: MOCK_PROVIDER.into(), model: Some(MOCK_MODEL.into()) }
    }

    fn engine(&self, provider: &str, model: &str) -> Result<Arc<dyn AgentEngine>, AgentError> {
        if provider != MOCK_PROVIDER {
            return Err(AgentError::Provider(format!("unknown provider `{provider}`")));
        }
        Self::engine_with(self.config, model)
    }

    fn models(&self, _provider: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
        Box::pin(async { Ok(vec![MOCK_MODEL.to_owned()]) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_catalog_builds_only_its_own_engine() {
        let c = MockCatalog::new(MockConfig::instant());
        let initial = c.initial();
        assert_eq!(initial.provider, MOCK_PROVIDER);
        assert_eq!(c.engine(MOCK_PROVIDER, MOCK_MODEL).map(|e| e.name().to_owned()), Ok("mock".to_owned()));
        assert!(c.engine("openai", MOCK_MODEL).is_err());
        assert!(c.engine(MOCK_PROVIDER, "gpt").is_err());
    }
}
