//! The providers Krune offers, read from the environment once at start.
//!
//! | Provider | Server | Credentials |
//! | --- | --- | --- |
//! | OpenAI | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
//! | OpenAI-compatible | `OPENAI_BASE_URL` (only listed when set) | `OPENAI_API_KEY`, if any |
//! | llama.cpp | `http://127.0.0.1:9931/v1` (`LLAMA_CPP_URL`) | none |
//! | Ollama | `http://localhost:11434/v1` | none |
//! | LM Studio | `http://localhost:1234/v1` | none |
//! | Simulated | the `MockAgent` | none |
//!
//! `KRUNE_PROVIDER` (a provider id) and `KRUNE_MODEL` choose what is selected at start.

use std::sync::Arc;

use krune_agent::{
    AgentEngine, AgentError, BoxFuture, EngineCatalog, LoginPrompt, MOCK_PROVIDER, MockCatalog, MockConfig, Provider,
    Selection,
};

use crate::ProviderError;
use crate::accounts::{Accounts, CHATGPT, COPILOT, Hosts};

use crate::http;
use crate::openai::{self, DEFAULT_BASE_URL, OpenAiConfig, OpenAiEngine};
use crate::secrets::{KeyringStore, SecretStore};

pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
pub const OLLAMA_URL: &str = "http://localhost:11434/v1";
pub const LM_STUDIO_URL: &str = "http://localhost:1234/v1";

/// llama-server's address as `llama-menu.bat` starts it (`--port 9931`); `LLAMA_CPP_URL`
/// overrides it (llama-server's own default is port 8080).
pub const LLAMA_CPP_URL: &str = "http://127.0.0.1:9931/v1";

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
        let accounts = Accounts::load(client.clone(), store, hosts);
        let key = var("OPENAI_API_KEY");
        let mut servers = vec![Server {
            provider: Provider {
                id: "openai".into(),
                label: "OpenAI".into(),
                default_model: Some(DEFAULT_OPENAI_MODEL.into()),
                missing: key.is_none().then(|| "needs OPENAI_API_KEY".into()),
                account: None,
            },
            base_url: DEFAULT_BASE_URL.into(),
            api_key: key.clone(),
            listing: Listing::ChatOnly,
        }];
        if let Some(url) = var("OPENAI_BASE_URL") {
            servers.push(Server {
                provider: Provider {
                    id: "custom".into(),
                    label: format!("OpenAI-compatible ({})", host_of(&url)),
                    default_model: None,
                    missing: None,
                    account: None,
                },
                base_url: url,
                api_key: key,
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
                    missing: None,
                    account: None,
                },
                base_url,
                api_key: None,
                listing,
            });
        }

        let ready = |id: &str| servers.iter().any(|s| s.provider.id == id && s.provider.missing.is_none());
        let known = |p: &str| p == MOCK_PROVIDER || Accounts::handles(p) || servers.iter().any(|s| s.provider.id == p);
        let provider = match var("KRUNE_PROVIDER").map(|p| p.to_ascii_lowercase()) {
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
        let initial = Selection { model: var("KRUNE_MODEL").or(default_model), provider };
        Ok(Catalog { servers, accounts, initial, client, mock: MockConfig::default() })
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
    /// OpenAI first, then the account providers, the other servers and the simulated agent.
    fn providers(&self) -> Vec<Provider> {
        let mut out: Vec<Provider> = self.servers.iter().map(|s| s.provider.clone()).collect();
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
            OpenAiEngine::with_client(Self::config(server, model), self.client.clone(), &server.provider.label)?;
        Ok(Arc::new(engine))
    }

    fn login(&self, provider: &str, prompt: LoginPrompt) -> BoxFuture<'static, Result<String, AgentError>> {
        self.accounts.login(provider, prompt)
    }

    fn logout(&self, provider: &str) -> BoxFuture<'static, Result<(), AgentError>> {
        self.accounts.logout(provider)
    }

    fn models(&self, provider: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
        if provider == MOCK_PROVIDER {
            return MockCatalog::default().models(provider);
        }
        if Accounts::handles(provider) {
            return self.accounts.models(provider);
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
        let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        let var = move |name: &str| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        Catalog::build(var, Arc::new(crate::secrets::MemoryStore::default()), Hosts::default())
    }

    fn ids(c: &Catalog) -> Vec<String> {
        c.providers().into_iter().map(|p| p.id).collect()
    }

    #[test]
    fn with_nothing_configured_the_simulated_agent_is_selected() {
        let c = catalog(&[]).unwrap();
        assert_eq!(ids(&c), ["openai", "chatgpt", "copilot", "llamacpp", "ollama", "lmstudio", "mock"]);
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
        assert_eq!(ids(&c), ["openai", "chatgpt", "copilot", "custom", "llamacpp", "ollama", "lmstudio", "mock"]);
        assert_eq!(c.initial(), Selection { provider: "custom".into(), model: Some("qwen3".into()) });
        assert_eq!(c.providers()[3].label, "OpenAI-compatible (gpu-box:8000)");
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
