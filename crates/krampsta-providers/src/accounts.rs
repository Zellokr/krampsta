//! Providers used through the user's own account instead of an API key — a ChatGPT plan and
//! GitHub Copilot: signing in and out, the stored sessions, and the engines and model lists
//! that need them.

use std::sync::{Arc, PoisonError, RwLock};

use krampsta_agent::{Account, AgentEngine, AgentError, BoxFuture, LoginPrompt, Provider, ProviderKind};
use serde::{Deserialize, Serialize};

use crate::chatgpt::{self, ChatGptAccess, OpenAiAccount};
use crate::copilot::{self, CopilotAccess, GitHub};
use crate::openai::OpenAiEngine;
use crate::responses::ResponsesEngine;
use crate::secrets::SecretStore;

pub const CHATGPT: &str = "chatgpt";
pub const COPILOT: &str = "copilot";
const CHATGPT_LABEL: &str = "ChatGPT";
const COPILOT_LABEL: &str = "GitHub Copilot";
const SIGN_IN_HINT: &str = "needs signing in (Settings › Accounts)";

/// The account services' hosts; tests point them at a fake server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hosts {
    pub github: GitHub,
    pub openai: OpenAiAccount,
}

/// What the keychain keeps for Copilot.
#[derive(Serialize, Deserialize)]
struct CopilotSecret {
    github_token: String,
    user: String,
}

/// A signed-in account: who, and how its requests are authorized.
struct Login<A> {
    user: String,
    access: Arc<A>,
}

type Slot<A> = Arc<RwLock<Option<Login<A>>>>;

fn read<A>(slot: &Slot<A>) -> Option<(String, Arc<A>)> {
    slot.read().unwrap_or_else(PoisonError::into_inner).as_ref().map(|l| (l.user.clone(), l.access.clone()))
}

fn write<A>(slot: &Slot<A>, login: Option<Login<A>>) {
    *slot.write().unwrap_or_else(PoisonError::into_inner) = login;
}

pub struct Accounts {
    client: reqwest::Client,
    store: Arc<dyn SecretStore>,
    hosts: Hosts,
    chatgpt: Slot<ChatGptAccess>,
    copilot: Slot<CopilotAccess>,
}

impl Accounts {
    /// Restores the sessions saved in `store`. An unreadable entry just means signed out.
    pub fn load(client: reqwest::Client, store: Arc<dyn SecretStore>, hosts: Hosts) -> Self {
        let saved = |key: &str| store.get(key).ok().flatten();
        let copilot = saved(COPILOT).and_then(|raw| serde_json::from_str::<CopilotSecret>(&raw).ok()).map(|s| Login {
            access: Arc::new(CopilotAccess::new(client.clone(), hosts.github.clone(), s.github_token)),
            user: s.user,
        });
        let chatgpt =
            saved(CHATGPT).and_then(|raw| serde_json::from_str::<chatgpt::Secret>(&raw).ok()).map(|s| Login {
                user: s.email.clone(),
                access: Arc::new(ChatGptAccess::new(
                    client.clone(),
                    hosts.openai.clone(),
                    store.clone(),
                    CHATGPT,
                    chatgpt::Session {
                        refresh_token: s.refresh_token,
                        account_id: s.account_id,
                        email: s.email,
                        access_token: None,
                    },
                )),
            });
        Accounts {
            client,
            store,
            hosts,
            chatgpt: Arc::new(RwLock::new(chatgpt)),
            copilot: Arc::new(RwLock::new(copilot)),
        }
    }

    pub fn handles(id: &str) -> bool {
        id == CHATGPT || id == COPILOT
    }

    pub fn signed_in(&self, id: &str) -> bool {
        match id {
            CHATGPT => read(&self.chatgpt).is_some(),
            COPILOT => read(&self.copilot).is_some(),
            _ => false,
        }
    }

    /// The account providers, as pickers list them.
    pub fn providers(&self) -> Vec<Provider> {
        let provider = |id: &str, label: &str, user: Option<String>| Provider {
            id: id.into(),
            label: label.into(),
            default_model: None,
            missing: user.is_none().then(|| SIGN_IN_HINT.into()),
            kind: ProviderKind::Account(match user {
                Some(user) => Account::SignedIn { user },
                None => Account::SignedOut,
            }),
            // Its Responses engine takes a reasoning effort; Copilot's chat API may reject one.
            reasoning_effort: id == CHATGPT,
        };
        vec![
            provider(CHATGPT, CHATGPT_LABEL, read(&self.chatgpt).map(|(u, _)| u)),
            provider(COPILOT, COPILOT_LABEL, read(&self.copilot).map(|(u, _)| u)),
        ]
    }

    pub fn engine(&self, id: &str, model: &str) -> Result<Arc<dyn AgentEngine>, AgentError> {
        let client = self.client.clone();
        match id {
            CHATGPT => {
                let (_, access) = read(&self.chatgpt).ok_or_else(|| needs_sign_in(id))?;
                Ok(Arc::new(ResponsesEngine::new(access.clone(), model, client, CHATGPT_LABEL).with_shapes(access)))
            }
            COPILOT => {
                let (_, access) = read(&self.copilot).ok_or_else(|| needs_sign_in(id))?;
                Ok(Arc::new(OpenAiEngine::with_access(access, model, client, COPILOT_LABEL)))
            }
            _ => Err(unknown(id)),
        }
    }

    pub fn models(&self, id: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
        let client = self.client.clone();
        let (chatgpt, copilot) = (read(&self.chatgpt), read(&self.copilot));
        let id = id.to_owned();
        Box::pin(async move {
            match id.as_str() {
                CHATGPT => chatgpt::list_models(&client, &chatgpt.ok_or_else(|| needs_sign_in(&id))?.1).await,
                COPILOT => copilot::list_models(&client, &copilot.ok_or_else(|| needs_sign_in(&id))?.1).await,
                _ => Err(unknown(&id)),
            }
        })
    }

    /// Signs in with the provider's device code, saves the session and resolves to the account.
    pub fn login(&self, id: &str, prompt: LoginPrompt) -> BoxFuture<'static, Result<String, AgentError>> {
        let (client, hosts, store) = (self.client.clone(), self.hosts.clone(), self.store.clone());
        let (chatgpt_slot, copilot_slot) = (self.chatgpt.clone(), self.copilot.clone());
        let id = id.to_owned();
        Box::pin(async move {
            match id.as_str() {
                CHATGPT => {
                    let session = chatgpt::device_login(&client, &hosts.openai, prompt).await?;
                    let secret = to_json(&chatgpt::Secret {
                        refresh_token: session.refresh_token.clone(),
                        account_id: session.account_id.clone(),
                        email: session.email.clone(),
                    })?;
                    let save = store.clone();
                    blocking(move || save.set(CHATGPT, &secret)).await?;
                    let user = session.email.clone();
                    let access = ChatGptAccess::new(client, hosts.openai, store, CHATGPT, session);
                    write(&chatgpt_slot, Some(Login { user: user.clone(), access: Arc::new(access) }));
                    Ok(user)
                }
                COPILOT => {
                    let token = copilot::device_login(&client, &hosts.github, prompt).await?;
                    let user = copilot::user_login(&client, &hosts.github, &token).await?;
                    let secret = to_json(&CopilotSecret { github_token: token.clone(), user: user.clone() })?;
                    blocking(move || store.set(COPILOT, &secret)).await?;
                    let access = CopilotAccess::new(client, hosts.github, token);
                    write(&copilot_slot, Some(Login { user: user.clone(), access: Arc::new(access) }));
                    Ok(user)
                }
                _ => Err(unknown(&id)),
            }
        })
    }

    /// Forgets the stored session.
    pub fn logout(&self, id: &str) -> BoxFuture<'static, Result<(), AgentError>> {
        let (store, chatgpt_slot, copilot_slot) = (self.store.clone(), self.chatgpt.clone(), self.copilot.clone());
        let id = id.to_owned();
        Box::pin(async move {
            match id.as_str() {
                CHATGPT => {
                    blocking(move || store.delete(CHATGPT)).await?;
                    write(&chatgpt_slot, None);
                }
                COPILOT => {
                    blocking(move || store.delete(COPILOT)).await?;
                    write(&copilot_slot, None);
                }
                _ => {}
            }
            Ok(())
        })
    }
}

fn label(id: &str) -> &str {
    match id {
        CHATGPT => CHATGPT_LABEL,
        COPILOT => COPILOT_LABEL,
        other => other,
    }
}

fn needs_sign_in(id: &str) -> AgentError {
    AgentError::Provider(format!("{} {SIGN_IN_HINT}", label(id)))
}

fn unknown(id: &str) -> AgentError {
    AgentError::Provider(format!("`{id}` has no account to sign in to"))
}

fn to_json(value: &impl Serialize) -> Result<String, AgentError> {
    serde_json::to_string(value).map_err(|e| AgentError::Provider(e.to_string()))
}

/// Runs a keychain call off the async workers (it may block on the OS).
async fn blocking(f: impl FnOnce() -> Result<(), String> + Send + 'static) -> Result<(), AgentError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AgentError::Provider(format!("the keychain call ended abnormally: {e}")))?
        .map_err(AgentError::Provider)
}
