//! Providers used through the user's own account instead of an API key: signing in and out,
//! the stored credentials, and the engines and model lists that need them.

use std::sync::{Arc, PoisonError, RwLock};

use krune_agent::{Account, AgentEngine, AgentError, BoxFuture, LoginPrompt, Provider};
use serde::{Deserialize, Serialize};

use crate::copilot::{self, CopilotAccess, GitHub};
use crate::openai::OpenAiEngine;
use crate::secrets::SecretStore;

pub const COPILOT: &str = "copilot";
const COPILOT_LABEL: &str = "GitHub Copilot";
const SIGN_IN_HINT: &str = "needs signing in (Settings › Accounts)";

/// What the keychain keeps for Copilot.
#[derive(Serialize, Deserialize)]
struct CopilotSecret {
    github_token: String,
    user: String,
}

struct CopilotLogin {
    user: String,
    access: Arc<CopilotAccess>,
}

type Slot<T> = Arc<RwLock<Option<T>>>;

pub struct Accounts {
    client: reqwest::Client,
    store: Arc<dyn SecretStore>,
    github: GitHub,
    copilot: Slot<CopilotLogin>,
}

impl Accounts {
    /// Restores the sessions saved in `store`. An unreadable entry just means signed out.
    pub fn load(client: reqwest::Client, store: Arc<dyn SecretStore>, github: GitHub) -> Self {
        let copilot = match store.get(COPILOT) {
            Ok(Some(raw)) => serde_json::from_str::<CopilotSecret>(&raw).ok().map(|s| CopilotLogin {
                access: Arc::new(CopilotAccess::new(client.clone(), github.clone(), s.github_token)),
                user: s.user,
            }),
            Ok(None) | Err(_) => None,
        };
        Accounts { client, store, github, copilot: Arc::new(RwLock::new(copilot)) }
    }

    pub fn handles(id: &str) -> bool {
        id == COPILOT
    }

    pub fn signed_in(&self, id: &str) -> bool {
        id == COPILOT && self.copilot_access().is_some()
    }

    fn copilot_access(&self) -> Option<Arc<CopilotAccess>> {
        self.copilot.read().unwrap_or_else(PoisonError::into_inner).as_ref().map(|l| l.access.clone())
    }

    /// The account providers, as pickers list them.
    pub fn providers(&self) -> Vec<Provider> {
        let user = self.copilot.read().unwrap_or_else(PoisonError::into_inner).as_ref().map(|l| l.user.clone());
        vec![Provider {
            id: COPILOT.into(),
            label: COPILOT_LABEL.into(),
            default_model: None,
            missing: user.is_none().then(|| SIGN_IN_HINT.into()),
            account: Some(match user {
                Some(user) => Account::SignedIn { user },
                None => Account::SignedOut,
            }),
        }]
    }

    pub fn engine(&self, id: &str, model: &str) -> Result<Arc<dyn AgentEngine>, AgentError> {
        let access = self.copilot_access().filter(|_| id == COPILOT).ok_or_else(|| needs_sign_in(id))?;
        Ok(Arc::new(OpenAiEngine::with_access(access, model, self.client.clone(), COPILOT_LABEL)))
    }

    pub fn models(&self, id: &str) -> BoxFuture<'static, Result<Vec<String>, AgentError>> {
        let (client, access) = (self.client.clone(), self.copilot_access().filter(|_| id == COPILOT));
        let id = id.to_owned();
        Box::pin(async move {
            let access = access.ok_or_else(|| needs_sign_in(&id))?;
            copilot::list_models(&client, &access).await
        })
    }

    /// Signs in with GitHub's device flow, saves the token and resolves to the GitHub login.
    pub fn login(&self, id: &str, prompt: LoginPrompt) -> BoxFuture<'static, Result<String, AgentError>> {
        let (client, github, store, slot) =
            (self.client.clone(), self.github.clone(), self.store.clone(), self.copilot.clone());
        let id = id.to_owned();
        Box::pin(async move {
            if id != COPILOT {
                return Err(AgentError::Provider(format!("`{id}` has no account to sign in to")));
            }
            let token = copilot::device_login(&client, &github, prompt).await?;
            let user = copilot::user_login(&client, &github, &token).await?;
            let secret = serde_json::to_string(&CopilotSecret { github_token: token.clone(), user: user.clone() })
                .map_err(|e| AgentError::Provider(e.to_string()))?;
            blocking(move || store.set(COPILOT, &secret)).await?;
            let access = Arc::new(CopilotAccess::new(client, github, token));
            *slot.write().unwrap_or_else(PoisonError::into_inner) = Some(CopilotLogin { user: user.clone(), access });
            Ok(user)
        })
    }

    /// Forgets the stored session.
    pub fn logout(&self, id: &str) -> BoxFuture<'static, Result<(), AgentError>> {
        let (store, slot, id) = (self.store.clone(), self.copilot.clone(), id.to_owned());
        Box::pin(async move {
            if id == COPILOT {
                blocking(move || store.delete(COPILOT)).await?;
                *slot.write().unwrap_or_else(PoisonError::into_inner) = None;
            }
            Ok(())
        })
    }
}

fn needs_sign_in(id: &str) -> AgentError {
    let label = if id == COPILOT { COPILOT_LABEL } else { id };
    AgentError::Provider(format!("{label} {SIGN_IN_HINT}"))
}

/// Runs a keychain call off the async workers (it may block on the OS).
async fn blocking(f: impl FnOnce() -> Result<(), String> + Send + 'static) -> Result<(), AgentError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AgentError::Provider(format!("the keychain call ended abnormally: {e}")))?
        .map_err(AgentError::Provider)
}
