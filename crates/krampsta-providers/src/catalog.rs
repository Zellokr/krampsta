//! The providers Krampsta offers, read from the environment once at start.
//!
//! | Provider | Server | Credentials |
//! | --- | --- | --- |
//! | OpenAI | `https://api.openai.com/v1` | saved in Settings (system keychain), else `OPENAI_API_KEY` |
//! | OpenAI-compatible | `OPENAI_BASE_URL` (only listed when set) | `OPENAI_API_KEY`, if any |
//! | llama.cpp | `http://127.0.0.1:9931/v1` (`LLAMA_CPP_URL`) | none |
//! | Ollama | `http://localhost:11434/v1` | none |
//! | LM Studio | `http://localhost:1234/v1` | none |
//! | Simulated | the `MockAgent` | none |
//!
//! `KRAMPSTA_PROVIDER` (a provider id) and `KRAMPSTA_MODEL` choose what is selected at start.

use std::sync::{Arc, PoisonError, RwLock};

use krampsta_agent::{
    AgentEngine, AgentError, BoxFuture, EffortLevel, EngineCatalog, KeySource, LoginPrompt, MOCK_PROVIDER, MockCatalog,
    MockConfig, Provider, ProviderKind, Selection,
};

use crate::ProviderError;
use crate::accounts::{self, Accounts, CHATGPT, COPILOT, Hosts};

use crate::http;
use crate::openai::{self, DEFAULT_BASE_URL, OpenAiConfig, OpenAiEngine};
use crate::secrets::{KeyringStore, SecretStore};

pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
pub const OLLAMA_URL: &str = "http://localhost:11434/v1";
pub const LM_STUDIO_URL: &str = "http://localhost:1234/v1";

/// llama-server's address as `llama-menu.bat` starts it (`--port 9931`); `LLAMA_CPP_URL`
/// overrides it (llama-server's own default is port 8080).
pub const LLAMA_CPP_URL: &str = "http://127.0.0.1:9931/v1";

/// The provider whose API key can be saved from the app, and its keychain entry.
const OPENAI: &str = "openai";
pub const OPENAI_KEY_ENTRY: &str = "openai-api-key";
const OPENAI_KEY_VAR: &str = "OPENAI_API_KEY";
const OPENAI_KEY_HINT: &str = "needs an API key (Settings › API keys, or OPENAI_API_KEY)";

/// OpenAI's key and where it comes from: the one saved in the app wins over the environment's.
fn openai_key(saved: Option<&str>, env: Option<&str>) -> (Option<String>, KeySource) {
    match (saved, env) {
        (Some(key), _) => (Some(key.to_owned()), KeySource::Saved),
        (None, Some(key)) => (Some(key.to_owned()), KeySource::Environment(OPENAI_KEY_VAR.into())),
        (None, None) => (None, KeySource::Missing),
    }
}

/// How a server's model list is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Listing {
    /// As the server names them.
    All,
    /// Without models that can't chat (OpenAI lists embeddings, speech… with the rest).
    ChatOnly,
    /// By file name: llama-server names a model by the path it was loaded from
    /// (`C:\…\Models\Qwen3.8-27B-UD-IQ3_XXS.gguf` shows as `Qwen3.8-27B-UD-IQ3_XXS`). It serves
    /// the one model it loaded whatever name a request carries, so the short name works.
    FileNames,
}

/// One OpenAI-protocol server.
#[derive(Clone)]
struct Server {
    provider: Provider,
    base_url: String,
    api_key: Option<String>,
    listing: Listing,
}

pub struct Catalog {
    servers: Vec<Server>,
    accounts: Accounts,
    initial: Selection,
    client: reqwest::Client,
    mock: MockConfig,
    store: Arc<dyn SecretStore>,
    /// OpenAI's key as saved from the app (and in `store`), and as `OPENAI_API_KEY` gave it.
    saved_key: Arc<RwLock<Option<String>>>,
    env_key: Option<String>,
}

impl Catalog {
    /// The catalog the environment describes, with the sessions saved in the system keychain.
    pub fn from_env() -> Result<Self, ProviderError> {
        Self::build(|name| std::env::var(name).ok(), Arc::new(KeyringStore), Hosts::default())
    }

    /// A catalog over chosen variables, secret store and account hosts (for tests).
    pub fn build(
        var: impl Fn(&str) -> Option<String>,
        store: Arc<dyn SecretStore>,
        hosts: Hosts,
    ) -> Result<Self, ProviderError> {
        let var = |name: &str| var(name).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let client = http::client()?;
        let accounts = Accounts::load(client.clone(), store.clone(), hosts);
        let key = var(OPENAI_KEY_VAR);
        // An unreadable entry means none saved; the environment's key (if any) still works.
        let saved = store.get(OPENAI_KEY_ENTRY).ok().flatten().filter(|k| !k.trim().is_empty());
        // OpenAI's key and `missing` are filled in by `resolve`, from the saved or environment key.
        let mut servers = vec![Server {
            provider: Provider {
                id: OPENAI.into(),
                label: "OpenAI".into(),
                default_model: Some(DEFAULT_OPENAI_MODEL.into()),
                missing: None,
                kind: ProviderKind::Cloud,
                reasoning_effort: false,
                images: false,
                api_key: None,
            },
            base_url: DEFAULT_BASE_URL.into(),
            api_key: None,
            listing: Listing::ChatOnly,
        }];
        if let Some(url) = var("OPENAI_BASE_URL") {
            servers.push(Server {
                provider: Provider {
                    id: "custom".into(),
                    label: format!("OpenAI-compatible ({})", host_of(&url)),
                    default_model: None,
                    missing: None,
                    reasoning_effort: false,
                    images: false,
                    kind: ProviderKind::Cloud,
                    api_key: None,
                },
                base_url: url,
                api_key: key.clone(),
                listing: Listing::All,
            });
        }
        let llama_cpp = var("LLAMA_CPP_URL").unwrap_or_else(|| LLAMA_CPP_URL.to_owned());
        let local = [
            ("llamacpp", "llama.cpp", llama_cpp, Listing::FileNames),
            ("ollama", "Ollama", OLLAMA_URL.to_owned(), Listing::All),
            ("lmstudio", "LM Studio", LM_STUDIO_URL.to_owned(), Listing::All),
        ];
        for (id, label, base_url, listing) in local {
            servers.push(Server {
                provider: Provider {
                    id: id.into(),
                    label: label.into(),
                    default_model: None,
                    reasoning_effort: false,
                    images: false,
                    missing: None,
                    kind: ProviderKind::Local,
                    api_key: None,
                },
                base_url,
                api_key: None,
                listing,
            });
        }

        let has_openai_key = openai_key(saved.as_deref(), key.as_deref()).0.is_some();
        let ready = |id: &str| match id {
            OPENAI => has_openai_key,
            _ => servers.iter().any(|s| s.provider.id == id && s.provider.missing.is_none()),
        };
        let known = |p: &str| p == MOCK_PROVIDER || Accounts::handles(p) || servers.iter().any(|s| s.provider.id == p);
        let provider = match var("KRAMPSTA_PROVIDER").map(|p| p.to_ascii_lowercase()) {
            Some(p) if known(&p) => p,
            Some(p) => return Err(ProviderError::Unknown(p)),
            None if ready("custom") => "custom".into(),
            None if ready("openai") => "openai".into(),
            None if accounts.signed_in(CHATGPT) => CHATGPT.into(),
            None if accounts.signed_in(COPILOT) => COPILOT.into(),
            None => MOCK_PROVIDER.into(),
        };
        let default_model = match servers.iter().find(|s| s.provider.id == provider) {
            Some(s) => s.provider.default_model.clone(),
            None if provider == MOCK_PROVIDER => MockCatalog::provider().default_model,
            None => None,
        };
        let initial = Selection { model: var("KRAMPSTA_MODEL").or(default_model), provider };
        Ok(Catalog {
            servers,
            accounts,
            initial,
            client,
            mock: MockConfig::default(),
            store,
            saved_key: Arc::new(RwLock::new(saved)),
            env_key: key,
        })
    }

    /// `id`'s server, with OpenAI's key as it stands now.
    fn server(&self, id: &str) -> Result<Server, AgentError> {
        self.servers
            .iter()
            .find(|s| s.provider.id == id)
            .map(|s| self.resolve(s))
            .ok_or_else(|| AgentError::Provider(format!("unknown provider `{id}`")))
    }

    /// A server as it stands: OpenAI's takes the saved key, else the environment's.
    fn resolve(&self, server: &Server) -> Server {
        let mut server = server.clone();
        if server.provider.id == OPENAI {
            let saved = self.saved_key.read().unwrap_or_else(PoisonError::into_inner);
            let (key, source) = openai_key(saved.as_deref(), self.env_key.as_deref());
            server.provider.missing = key.is_none().then(|| OPENAI_KEY_HINT.into());
            server.provider.api_key = Some(source);
            server.api_key = key;
        }
        server
    }

    fn config(server: &Server, model: &str) -> OpenAiConfig {
        OpenAiConfig { base_url: server.base_url.clone(), api_key: server.api_key.clone(), model: model.to_owned() }
    }
}

impl EngineCatalog for Catalog {
    /// OpenAI first, then the account providers, the other servers and the simulated agent.
    fn providers(&self) -> Vec<Provider> {
        let mut out: Vec<Provider> = self.servers.iter().map(|s| self.resolve(s).provider).collect();
        out.splice(1..1, self.accounts.providers());
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
        if Accounts::handles(provider) {
            return self.accounts.engine(provider, model);
        }
        let server = self.server(provider)?;
        if let Some(missing) = &server.provider.missing {
            return Err(AgentError::Provider(format!("{} {missing}", server.provider.label)));
        }
        let engine =
            OpenAiEngine::with_client(Self::config(&server, model), self.client.clone(), &server.provider.label)?;
        Ok(Arc::new(engine))
    }

    fn set_api_key(&self, provider: &str, key: Option<String>) -> BoxFuture<'static, Result<(), AgentError>> {
        if provider != OPENAI {
            let error = AgentError::Provider(format!("`{provider}` takes no API key"));
            return Box::pin(async move { Err(error) });
        }
        let key = key.map(|k| k.trim().to_owned());
        let (store, slot) = (self.store.clone(), self.saved_key.clone());
        Box::pin(async move {
            match &key {
                Some(k) if k.is_empty() => return Err(AgentError::Provider("The API key is empty.".into())),
                Some(k) => {
                    let k = k.clone();
                    accounts::blocking(move || store.set(OPENAI_KEY_ENTRY, &k)).await?;
                }
                None => accounts::blocking(move || store.delete(OPENAI_KEY_ENTRY)).await?,
            }
            *slot.write().unwrap_or_else(PoisonError::into_inner) = key;
            Ok(())
        })
    }

    fn login(&self, provider: &str, prompt: LoginPrompt) -> BoxFuture<'static, Result<String, AgentError>> {
        self.accounts.login(provider, prompt)
    }

    fn logout(&self, provider: &str) -> BoxFuture<'static, Result<(), AgentError>> {
        self.accounts.logout(provider)
    }

    fn efforts(&self, provider: &str, model: &str) -> Vec<EffortLevel> {
        self.accounts.efforts(provider, model)
    }

    fn models(&self, provider: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
        if provider == MOCK_PROVIDER {
            return MockCatalog::default().models(provider);
        }
        if Accounts::handles(provider) {
            return self.accounts.models(provider);
        }
        let server = match self.server(provider) {
            Ok(s) if s.provider.missing.is_none() => s,
            Ok(s) => {
                let why = format!("{} {}", s.provider.label, s.provider.missing.clone().unwrap_or_default());
                return Box::pin(async move { Err(AgentError::Provider(why)) });
            }
            Err(e) => return Box::pin(async move { Err(e) }),
        };
        let client = self.client.clone();
        Box::pin(async move {
            let config = Self::config(&server, "");
            let models = openai::list_models(&client, &config, server.listing == Listing::ChatOnly).await?;
            Ok(match server.listing {
                Listing::FileNames => {
                    let mut names: Vec<String> = models.iter().map(|id| file_name(id).to_owned()).collect();
                    names.sort();
                    names.dedup();
                    names
                }
                Listing::All | Listing::ChatOnly => models,
            })
        })
    }
}

/// A model file's name without folders or `.gguf`; other ids come back unchanged.
fn file_name(id: &str) -> &str {
    let name = id.rsplit(['/', '\\']).next().unwrap_or(id);
    match name.len().checked_sub(5) {
        Some(cut) if name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(".gguf") => &name[..cut],
        _ => name,
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
        catalog_with(pairs, Arc::new(crate::secrets::MemoryStore::default()))
    }

    fn catalog_with(pairs: &[(&str, &str)], store: Arc<dyn SecretStore>) -> Result<Catalog, ProviderError> {
        let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        let var = move |name: &str| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        Catalog::build(var, store, Hosts::default())
    }

    fn openai(c: &Catalog) -> Provider {
        c.providers().into_iter().find(|p| p.id == OPENAI).unwrap()
    }

    fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(fut)
    }

    #[test]
    fn a_key_saved_in_the_app_wins_over_the_environment_and_removing_it_falls_back() {
        let store = Arc::new(crate::secrets::MemoryStore::default());
        let c = catalog_with(&[], store.clone()).unwrap();
        assert_eq!(openai(&c).api_key, Some(KeySource::Missing));

        block_on(c.set_api_key(OPENAI, Some("  sk-saved \n".into()))).unwrap();
        assert_eq!(store.get(OPENAI_KEY_ENTRY).unwrap().as_deref(), Some("sk-saved"), "trimmed, in the keychain");
        assert_eq!(openai(&c).api_key, Some(KeySource::Saved));
        assert_eq!(openai(&c).missing, None);
        assert_eq!(c.server(OPENAI).unwrap().api_key.as_deref(), Some("sk-saved"));
        assert!(c.engine(OPENAI, "gpt-4o").is_ok());

        // After a restart the saved key is still there, and beats the environment's.
        let c = catalog_with(&[("OPENAI_API_KEY", "sk-env")], store.clone()).unwrap();
        assert_eq!(c.initial().provider, OPENAI);
        assert_eq!(c.server(OPENAI).unwrap().api_key.as_deref(), Some("sk-saved"));

        block_on(c.set_api_key(OPENAI, None)).unwrap();
        assert_eq!(store.get(OPENAI_KEY_ENTRY).unwrap(), None);
        assert_eq!(openai(&c).api_key, Some(KeySource::Environment("OPENAI_API_KEY".into())));
        assert_eq!(c.server(OPENAI).unwrap().api_key.as_deref(), Some("sk-env"));
    }

    #[test]
    fn only_openai_takes_a_key_and_an_empty_one_is_refused() {
        let c = catalog(&[]).unwrap();
        let err = block_on(c.set_api_key(OPENAI, Some("   ".into()))).unwrap_err();
        assert_eq!(err, AgentError::Provider("The API key is empty.".into()));
        assert!(block_on(c.set_api_key("ollama", Some("x".into()))).is_err());
        assert!(c.providers().iter().filter(|p| p.id != OPENAI).all(|p| p.api_key.is_none()));
    }

    #[test]
    fn debug_never_prints_a_saved_key() {
        let c = catalog(&[]).unwrap();
        block_on(c.set_api_key(OPENAI, Some("sk-secret".into()))).unwrap();
        assert!(!format!("{:?}", c.providers()).contains("sk-secret"));
    }

    fn ids(c: &Catalog) -> Vec<String> {
        c.providers().into_iter().map(|p| p.id).collect()
    }

    #[test]
    fn with_nothing_configured_the_simulated_agent_is_selected() {
        let c = catalog(&[]).unwrap();
        assert_eq!(ids(&c), ["openai", "chatgpt", "copilot", "llamacpp", "ollama", "lmstudio", "mock"]);
        assert_eq!(c.initial(), MockCatalog::default().initial());
        assert_eq!(c.providers()[0].missing.as_deref(), Some(OPENAI_KEY_HINT));
        let err = c.engine("openai", "gpt-4o-mini").err().unwrap();
        assert!(err.to_string().contains("OpenAI needs an API key"), "{err}");
    }

    #[test]
    fn a_key_selects_openai_and_a_base_url_adds_and_selects_a_custom_server() {
        let c = catalog(&[("OPENAI_API_KEY", "sk-test")]).unwrap();
        assert_eq!(c.initial(), Selection { provider: "openai".into(), model: Some(DEFAULT_OPENAI_MODEL.into()) });
        assert_eq!(c.engine("openai", "gpt-4o").unwrap().name(), "OpenAI · gpt-4o");

        let c = catalog(&[("OPENAI_BASE_URL", "http://gpu-box:8000/v1"), ("KRAMPSTA_MODEL", "qwen3")]).unwrap();
        assert_eq!(ids(&c), ["openai", "chatgpt", "copilot", "custom", "llamacpp", "ollama", "lmstudio", "mock"]);
        assert_eq!(c.initial(), Selection { provider: "custom".into(), model: Some("qwen3".into()) });
        assert_eq!(c.providers()[3].label, "OpenAI-compatible (gpu-box:8000)");
    }

    #[test]
    fn krampsta_provider_picks_the_start_selection() {
        let c = catalog(&[("KRAMPSTA_PROVIDER", "Ollama"), ("KRAMPSTA_MODEL", "llama3.2")]).unwrap();
        assert_eq!(c.initial(), Selection { provider: "ollama".into(), model: Some("llama3.2".into()) });
        assert_eq!(c.engine("ollama", "llama3.2").unwrap().name(), "Ollama · llama3.2");
        let c = catalog(&[("KRAMPSTA_PROVIDER", "lmstudio")]).unwrap();
        assert_eq!(c.initial().model, None, "local servers have no default model until listed");
        assert!(matches!(catalog(&[("KRAMPSTA_PROVIDER", "nope")]), Err(ProviderError::Unknown(p)) if p == "nope"));
    }

    #[test]
    fn model_files_show_by_name() {
        assert_eq!(file_name(r"C:\Users\me\Desktop\Models\Qwen3.8-27B-UD-IQ3_XXS.gguf"), "Qwen3.8-27B-UD-IQ3_XXS");
        assert_eq!(file_name("/models/llama-3.2-3b.Q4_K_M.GGUF"), "llama-3.2-3b.Q4_K_M");
        assert_eq!(file_name("qwen3:4b"), "qwen3:4b");
        assert_eq!(file_name("gguf"), "gguf");
    }

    #[test]
    fn llama_cpp_defaults_to_the_llama_menu_port_and_can_be_moved() {
        let c = catalog(&[]).unwrap();
        assert_eq!(c.server("llamacpp").unwrap().base_url, LLAMA_CPP_URL);
        assert_eq!(c.engine("llamacpp", "Qwen3.8-27B").unwrap().name(), "llama.cpp · Qwen3.8-27B");
        let c = catalog(&[("LLAMA_CPP_URL", "http://127.0.0.1:8080/v1")]).unwrap();
        assert_eq!(c.server("llamacpp").unwrap().base_url, "http://127.0.0.1:8080/v1");
    }

    #[test]
    fn llama_server_models_are_listed_by_file_name() {
        use std::io::{Read, Write};
        // What llama-server answers when started with `-m <full path>` (as llama-menu.bat does).
        let body = r#"{"object":"list","data":[{"id":"C:\\Users\\me\\Desktop\\Models\\Qwen3.8-27B-UD-IQ3_XXS.gguf","object":"model","owned_by":"llamacpp"}]}"#;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0; 4096];
            let _ = stream.read(&mut buf);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
        });
        let c = catalog(&[("LLAMA_CPP_URL", url.as_str())]).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert_eq!(rt.block_on(c.models("llamacpp")).unwrap(), ["Qwen3.8-27B-UD-IQ3_XXS"]);
    }
}
