//! A ChatGPT plan (Plus, Pro, Team…) used through the user's own account, the way OpenAI's
//! Codex CLI signs in: OpenAI's device-code sign-in, the tokens it yields, their refresh, and
//! the headers Codex's backend expects. Not a documented public API, so all of it lives here.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use krampsta_agent::{AgentError, BoxFuture, DeviceCode, LoginPrompt};
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::access::{Access, Endpoint, bearer};
use crate::http;
use crate::secrets::SecretStore;

/// The OAuth app OpenAI's Codex CLI signs in with.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// The Codex version sent with the model list (the server filters models by it).
const CLIENT_VERSION: &str = "0.170.0";
const ORIGINATOR: &str = "codex_cli_rs";
/// Refresh the access token this long before it expires.
const REFRESH_MARGIN: i64 = 120;
/// How long the user has to approve the code.
const DEVICE_WAIT: Duration = Duration::from_secs(15 * 60);
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);

/// OpenAI's hosts; tests point them at a fake server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiAccount {
    /// `https://auth.openai.com` (sign-in, tokens).
    pub issuer: String,
    /// `https://chatgpt.com/backend-api/codex` (responses, models).
    pub api: String,
}

impl Default for OpenAiAccount {
    fn default() -> Self {
        OpenAiAccount { issuer: "https://auth.openai.com".into(), api: "https://chatgpt.com/backend-api/codex".into() }
    }
}

/// What a sign-in leaves: the long-lived part is stored, the access token kept in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub refresh_token: String,
    pub account_id: String,
    /// The account's e-mail, to show who is signed in.
    pub email: String,
    pub access_token: Option<String>,
}

/// What the keychain keeps for ChatGPT.
#[derive(Serialize, Deserialize)]
pub struct Secret {
    pub refresh_token: String,
    pub account_id: String,
    pub email: String,
}

#[derive(Deserialize)]
struct UserCode {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    /// Seconds, sent as a string.
    #[serde(default)]
    interval: Option<Value>,
}

#[derive(Deserialize)]
struct Approved {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Provider(message.into())
}

/// Signs in with OpenAI's device code: asks for a code, hands it to `prompt`, waits for the
/// user to approve it, then trades the approval for tokens. Cancel by dropping it.
pub async fn device_login(
    client: &reqwest::Client,
    hosts: &OpenAiAccount,
    prompt: LoginPrompt,
) -> Result<Session, AgentError> {
    let accounts = format!("{}/api/accounts", hosts.issuer);
    let reply = client
        .post(format!("{accounts}/deviceauth/usercode"))
        .json_body(&serde_json::json!({ "client_id": CLIENT_ID }))?
        .send()
        .await
        .map_err(|e| fail(format!("could not reach OpenAI: {e}")))?;
    let status = reply.status();
    let body = reply.text().await.map_err(|e| fail(e.to_string()))?;
    if status == StatusCode::NOT_FOUND {
        return Err(fail("OpenAI does not offer device sign-in for this account"));
    }
    if !status.is_success() {
        return Err(http::status_error(status, &body).into());
    }
    let code: UserCode = serde_json::from_str(&body)
        .map_err(|e| fail(format!("OpenAI sent a sign-in code Krampsta could not read ({e})")))?;
    let interval = code
        .interval
        .as_ref()
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .unwrap_or(5)
        .max(1);
    prompt(DeviceCode { url: format!("{}/codex/device", hosts.issuer), code: code.user_code.clone() });

    let started = tokio::time::Instant::now();
    let approved = loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let reply = client
            .post(format!("{accounts}/deviceauth/token"))
            .json_body(&serde_json::json!({ "device_auth_id": code.device_auth_id, "user_code": code.user_code }))?
            .send()
            .await
            .map_err(|e| fail(format!("could not reach OpenAI: {e}")))?;
        let status = reply.status();
        let body = reply.text().await.map_err(|e| fail(e.to_string()))?;
        if status.is_success() {
            break serde_json::from_str::<Approved>(&body)
                .map_err(|e| fail(format!("OpenAI sent an approval Krampsta could not read ({e})")))?;
        }
        // Not approved yet.
        if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
            if started.elapsed() > DEVICE_WAIT {
                return Err(fail("the code expired before it was approved; sign in again"));
            }
            continue;
        }
        return Err(http::status_error(status, &body).into());
    };

    let tokens = token_request(
        client,
        hosts,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", approved.authorization_code.as_str()),
            ("redirect_uri", &format!("{}/deviceauth/callback", hosts.issuer)),
            ("code_verifier", approved.code_verifier.as_str()),
        ],
    )
    .await?;
    let claims = tokens.id_token.as_deref().map(jwt_claims).unwrap_or(Value::Null);
    let account_id = claims["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .ok_or_else(|| fail("this OpenAI account has no ChatGPT plan to use"))?
        .to_owned();
    Ok(Session {
        refresh_token: tokens.refresh_token.ok_or_else(|| fail("OpenAI sent no refresh token"))?,
        account_id,
        email: claims["email"].as_str().unwrap_or("ChatGPT account").to_owned(),
        access_token: Some(tokens.access_token),
    })
}

async fn token_request(
    client: &reqwest::Client,
    hosts: &OpenAiAccount,
    form: &[(&str, &str)],
) -> Result<Tokens, AgentError> {
    let reply = client
        .post(format!("{}/oauth/token", hosts.issuer))
        .form(form)
        .timeout(LOOKUP_TIMEOUT)
        .send()
        .await
        .map_err(|e| fail(format!("could not reach OpenAI: {e}")))?;
    let status = reply.status();
    let body = reply.text().await.map_err(|e| fail(e.to_string()))?;
    if status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED {
        return Err(fail("OpenAI no longer accepts this sign-in; sign in again in Settings"));
    }
    if !status.is_success() {
        return Err(http::status_error(status, &body).into());
    }
    serde_json::from_str(&body).map_err(|e| fail(format!("OpenAI sent tokens Krampsta could not read ({e})")))
}

/// The claims of a JWT, without checking its signature (it came straight from OpenAI over
/// TLS and only says who the user is). `Null` when it isn't one.
fn jwt_claims(jwt: &str) -> Value {
    let Some(payload) = jwt.split('.').nth(1) else { return Value::Null };
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    engine
        .decode(payload.trim_end_matches('='))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// A token's expiry (`exp`), or an hour from now when it doesn't say.
fn expiry(access_token: &str) -> i64 {
    jwt_claims(access_token)["exp"].as_i64().unwrap_or_else(|| now() + 3600)
}

struct Current {
    session: Session,
    expires_at: i64,
}

/// Keeps the access token fresh, saving a rotated refresh token back to the keychain.
pub struct ChatGptAccess {
    client: reqwest::Client,
    hosts: OpenAiAccount,
    store: Arc<dyn SecretStore>,
    key: &'static str,
    current: Mutex<Current>,
}

impl ChatGptAccess {
    pub fn new(
        client: reqwest::Client,
        hosts: OpenAiAccount,
        store: Arc<dyn SecretStore>,
        key: &'static str,
        session: Session,
    ) -> Self {
        let expires_at = session.access_token.as_deref().map_or(0, expiry);
        ChatGptAccess { client, hosts, store, key, current: Mutex::new(Current { session, expires_at }) }
    }

    pub fn api(&self) -> &str {
        &self.hosts.api
    }

    async fn token(&self) -> Result<(String, String), AgentError> {
        let mut current = self.current.lock().await;
        if let Some(token) =
            current.session.access_token.clone().filter(|_| current.expires_at - REFRESH_MARGIN > now())
        {
            return Ok((token, current.session.account_id.clone()));
        }
        let tokens = token_request(
            &self.client,
            &self.hosts,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", current.session.refresh_token.as_str()),
            ],
        )
        .await?;
        if let Some(rotated) = tokens.refresh_token.filter(|r| *r != current.session.refresh_token) {
            current.session.refresh_token = rotated;
            let secret = serde_json::to_string(&Secret {
                refresh_token: current.session.refresh_token.clone(),
                account_id: current.session.account_id.clone(),
                email: current.session.email.clone(),
            })
            .map_err(|e| fail(e.to_string()))?;
            let (store, key) = (self.store.clone(), self.key);
            tokio::task::spawn_blocking(move || store.set(key, &secret))
                .await
                .map_err(|e| fail(e.to_string()))?
                .map_err(fail)?;
        }
        current.expires_at = expiry(&tokens.access_token);
        current.session.access_token = Some(tokens.access_token.clone());
        Ok((tokens.access_token, current.session.account_id.clone()))
    }
}

impl Access for ChatGptAccess {
    fn endpoint(&self) -> BoxFuture<'_, Result<Endpoint, AgentError>> {
        Box::pin(async move {
            let (token, account) = self.token().await?;
            Ok(Endpoint { base_url: self.hosts.api.clone(), headers: headers(&token, &account)? })
        })
    }
}

fn headers(token: &str, account: &str) -> Result<HeaderMap, AgentError> {
    let mut h = HeaderMap::new();
    h.insert(reqwest::header::AUTHORIZATION, bearer(token)?);
    let account = HeaderValue::from_str(account).map_err(|_| fail("the ChatGPT account id is not a valid header"))?;
    h.insert(HeaderName::from_static("chatgpt-account-id"), account);
    h.insert(HeaderName::from_static("originator"), HeaderValue::from_static(ORIGINATOR));
    h.insert(HeaderName::from_static("openai-beta"), HeaderValue::from_static("responses=experimental"));
    Ok(h)
}

/// The models this plan may use, as Codex lists them (`visibility: list`).
pub async fn list_models(client: &reqwest::Client, access: &ChatGptAccess) -> Result<Vec<String>, AgentError> {
    let endpoint = access.endpoint().await?;
    let url = format!("{}?client_version={CLIENT_VERSION}", endpoint.url("models"));
    let v = http::get_json(client.get(url).headers(endpoint.headers), LOOKUP_TIMEOUT).await?;
    Ok(parse_models(&v))
}

fn parse_models(v: &Value) -> Vec<String> {
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"].as_str().is_none_or(|vis| vis == "list"))
        .filter_map(|m| m["slug"].as_str().map(str::to_owned))
        .collect()
}

/// `RequestBuilder::json` without reqwest's `json` feature.
trait JsonBody: Sized {
    fn json_body(self, body: &Value) -> Result<Self, AgentError>;
}

impl JsonBody for reqwest::RequestBuilder {
    fn json_body(self, body: &Value) -> Result<Self, AgentError> {
        let bytes = serde_json::to_vec(body).map_err(|e| fail(e.to_string()))?;
        Ok(self.header("content-type", "application/json").timeout(LOOKUP_TIMEOUT).body(bytes))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A JWT whose payload is `claims` (unsigned: Krampsta never checks the signature).
    pub fn jwt(claims: &Value) -> String {
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("e30.{}.sig", engine.encode(claims.to_string()))
    }

    #[test]
    fn claims_come_out_of_the_id_token() {
        let token =
            jwt(&json!({"email": "me@example.com", "https://api.openai.com/auth": {"chatgpt_account_id": "acc-1"}}));
        let claims = jwt_claims(&token);
        assert_eq!(claims["email"], "me@example.com");
        assert_eq!(claims["https://api.openai.com/auth"]["chatgpt_account_id"], "acc-1");
        assert_eq!(jwt_claims("not a jwt"), Value::Null);
        assert_eq!(expiry(&jwt(&json!({"exp": 1_900_000_000}))), 1_900_000_000);
    }

    #[test]
    fn requests_carry_the_account_and_codexs_headers() {
        let h = headers("tok", "acc-1").unwrap();
        assert_eq!(h["authorization"], "Bearer tok");
        assert_eq!(h["chatgpt-account-id"], "acc-1");
        assert_eq!(h["originator"], ORIGINATOR);
    }

    #[test]
    fn only_listed_models_are_offered() {
        let v = json!({"models": [
            {"slug": "gpt-6-sol", "visibility": "list"},
            {"slug": "codex-auto-review", "visibility": "hide"},
            {"slug": "gpt-5.5"}
        ]});
        assert_eq!(parse_models(&v), ["gpt-6-sol", "gpt-5.5"]);
    }
}
