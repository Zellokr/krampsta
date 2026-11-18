//! The providers Krune offers, read from the environment once at start.
//!
//! | Provider | Server | Credentials |
//! | --- | --- | --- |
//! | OpenAI | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
//! | OpenAI-compatible | `OPENAI_BASE_URL` (only listed when set) | `OPENAI_API_KEY`, if any |
//! | Ollama | `http://localhost:11434/v1` | none |
//! | LM Studio | `http://localhost:1234/v1` | none |
//! | Simulated | the `MockAgent` | none |
//!
//! `KRUNE_PROVIDER` (a provider id) and `KRUNE_MODEL` choose what is selected at start.

use std::sync::Arc;

use krune_agent::{
    AgentEngine, AgentError, BoxFuture, EngineCatalog, MOCK_PROVIDER, MockCatalog, MockConfig, Provider, Selection,
};

use crate::ProviderError;
use crate::http;
use crate::openai::{self, DEFAULT_BASE_URL, OpenAiConfig, OpenAiEngine};

pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
pub const OLLAMA_URL: &str = "http://localhost:11434/v1";
pub const LM_STUDIO_URL: &str = "http://localhost:1234/v1";

/// One OpenAI-protocol server.
#[derive(Clone)]
struct Server {
    provider: Provider,
    base_url: String,
    api_key: Option<String>,
    /// Hide non-chat models from the list (OpenAI lists embeddings, speech… with the rest).
    chat_only: bool,
}

pub struct Catalog {
    servers: Vec<Server>,
    initial: Selection,
    client: reqwest::Client,
    mock: MockConfig,
}

impl Catalog {
    /// The catalog the environment describes.
    pub fn from_env() -> Result<Self, ProviderError> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Self, ProviderError> {
        let var = |name: &str| var(name).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let key = var("OPENAI_API_KEY");
        let mut servers = vec![Server {
            provider: Provider {
                id: "openai".into(),
                label: "OpenAI".into(),
                default_model: Some(DEFAULT_OPENAI_MODEL.into()),
                missing: key.is_none().then(|| "needs OPENAI_API_KEY".into()),
            },
            base_url: DEFAULT_BASE_URL.into(),
            api_key: key.clone(),
            chat_only: true,
        }];
        if let Some(url) = var("OPENAI_BASE_URL") {
            servers.push(Server {
                provider: Provider {
                    id: "custom".into(),
                    label: format!("OpenAI-compatible ({})", host_of(&url)),
                    default_model: None,
                    missing: None,
                },
                base_url: url,
                api_key: key,
                chat_only: false,
            });
        }
        for (id, label, url) in [("ollama", "Ollama", OLLAMA_URL), ("lmstudio", "LM Studio", LM_STUDIO_URL)] {
            servers.push(Server {
                provider: Provider { id: id.into(), label: label.into(), default_model: None, missing: None },
                base_url: url.into(),
                api_key: None,
                chat_only: false,
            });
        }

        let ready = |id: &str| servers.iter().any(|s| s.provider.id == id && s.provider.missing.is_none());
        let provider = match var("KRUNE_PROVIDER").map(|p| p.to_ascii_lowercase()) {
            Some(p) if p == MOCK_PROVIDER || servers.iter().any(|s| s.provider.id == p) => p,
            Some(p) => return Err(ProviderError::Unknown(p)),
            None if ready("custom") => "custom".into(),
            None if ready("openai") => "openai".into(),
            None => MOCK_PROVIDER.into(),
        };
        let default_model = servers
            .iter()
            .find(|s| s.provider.id == provider)
            .map_or_else(|| MockCatalog::provider().default_model, |s| s.provider.default_model.clone());
        let initial = Selection { model: var("KRUNE_MODEL").or(default_model), provider };
        Ok(Catalog { servers, initial, client: http::client()?, mock: MockConfig::default() })
    }

    fn server(&self, id: &str) -> Result<&Server, AgentError> {
        self.servers
            .iter()
            .find(|s| s.provider.id == id)
            .ok_or_else(|| AgentError::Provider(format!("unknown provider `{id}`")))
    }

    fn config(server: &Server, model: &str) -> OpenAiConfig {
        OpenAiConfig { base_url: server.base_url.clone(), api_key: server.api_key.clone(), model: model.to_owned() }
    }
}

impl EngineCatalog for Catalog {
    fn providers(&self) -> Vec<Provider> {
        let mut out: Vec<Provider> = self.servers.iter().map(|s| s.provider.clone()).collect();
        out.push(MockCatalog::provider());
        out
    }

    fn initial(&self) -> Selection {
        self.initial.clone()
    }

    fn engine(&self, provider: &str, model: &str) -> Result<Arc<dyn AgentEngine>, AgentError> {
        if provider == MOCK_PROVIDER {
            return MockCatalog::engine_with(self.mock, model);
        }
        let server = self.server(provider)?;
        if let Some(missing) = &server.provider.missing {
            return Err(AgentError::Provider(format!("{} {missing}", server.provider.label)));
        }
        let engine =
            OpenAiEngine::with_client(Self::config(server, model), self.client.clone(), &server.provider.label);
        Ok(Arc::new(engine))
    }

    fn models(&self, provider: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
        if provider == MOCK_PROVIDER {
            return MockCatalog::default().models(provider);
        }
        let server = match self.server(provider) {
            Ok(s) if s.provider.missing.is_none() => s.clone(),
            Ok(s) => {
                let why = format!("{} {}", s.provider.label, s.provider.missing.clone().unwrap_or_default());
                return Box::pin(async move { Err(AgentError::Provider(why)) });
            }
            Err(e) => return Box::pin(async move { Err(e) }),
        };
        let client = self.client.clone();
        Box::pin(async move {
            let config = Self::config(&server, "");
            Ok(openai::list_models(&client, &config, server.chat_only).await?)
        })
    }
}

/// `host:port` of a URL, for a short label.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(pairs: &[(&str, &str)]) -> Result<Catalog, ProviderError> {
        let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        Catalog::from_vars(move |name| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
    }

    fn ids(c: &Catalog) -> Vec<String> {
        c.providers().into_iter().map(|p| p.id).collect()
    }

    #[test]
    fn with_nothing_configured_the_simulated_agent_is_selected() {
        let c = catalog(&[]).unwrap();
        assert_eq!(ids(&c), ["openai", "ollama", "lmstudio", "mock"]);
        assert_eq!(c.initial(), MockCatalog::default().initial());
        assert_eq!(c.providers()[0].missing.as_deref(), Some("needs OPENAI_API_KEY"));
        let err = c.engine("openai", "gpt-4o-mini").err().unwrap();
        assert!(err.to_string().contains("OpenAI needs OPENAI_API_KEY"), "{err}");
    }

    #[test]
    fn a_key_selects_openai_and_a_base_url_adds_and_selects_a_custom_server() {
        let c = catalog(&[("OPENAI_API_KEY", "sk-test")]).unwrap();
        assert_eq!(c.initial(), Selection { provider: "openai".into(), model: Some(DEFAULT_OPENAI_MODEL.into()) });
        assert_eq!(c.engine("openai", "gpt-4o").unwrap().name(), "OpenAI · gpt-4o");

        let c = catalog(&[("OPENAI_BASE_URL", "http://gpu-box:8000/v1"), ("KRUNE_MODEL", "qwen3")]).unwrap();
        assert_eq!(ids(&c), ["openai", "custom", "ollama", "lmstudio", "mock"]);
        assert_eq!(c.initial(), Selection { provider: "custom".into(), model: Some("qwen3".into()) });
        assert_eq!(c.providers()[1].label, "OpenAI-compatible (gpu-box:8000)");
    }

    #[test]
    fn krune_provider_picks_the_start_selection() {
        let c = catalog(&[("KRUNE_PROVIDER", "Ollama"), ("KRUNE_MODEL", "llama3.2")]).unwrap();
        assert_eq!(c.initial(), Selection { provider: "ollama".into(), model: Some("llama3.2".into()) });
        assert_eq!(c.engine("ollama", "llama3.2").unwrap().name(), "Ollama · llama3.2");
        let c = catalog(&[("KRUNE_PROVIDER", "lmstudio")]).unwrap();
        assert_eq!(c.initial().model, None, "local servers have no default model until listed");
        assert!(matches!(catalog(&[("KRUNE_PROVIDER", "nope")]), Err(ProviderError::Unknown(p)) if p == "nope"));
    }
}
