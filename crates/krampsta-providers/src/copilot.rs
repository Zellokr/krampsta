//! GitHub Copilot through the user's own subscription: GitHub's OAuth device flow, the
//! short-lived Copilot token it unlocks, the headers Copilot expects and its model list.
//!
//! `copilot_internal` and these headers are what Copilot's editor clients use, not a documented
//! public API, so everything that depends on them lives in this file (adapted from Rugents).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use krampsta_agent::{AgentError, BoxFuture, DeviceCode, LoginPrompt};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::access::{Access, Endpoint, bearer};
use crate::http;

/// The OAuth app Copilot's editor integrations sign in with.
pub const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const DEFAULT_API: &str = "https://api.githubcopilot.com";
const EDITOR_VERSION: &str = "vscode/1.99.3";
const PLUGIN_VERSION: &str = "copilot-chat/0.26.7";
const UA: &str = "GitHubCopilotChat/0.26.7";
/// Exchange the Copilot token again this long before it expires.
const REFRESH_MARGIN: i64 = 60;
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);

/// GitHub's hosts; tests point them at a fake server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHub {
    /// `https://github.com` (device flow).
    pub web: String,
    /// `https://api.github.com` (user, Copilot token).
    pub api: String,
}

impl Default for GitHub {
    fn default() -> Self {
        GitHub { web: "https://github.com".into(), api: "https://api.github.com".into() }
    }
}

#[derive(Deserialize)]
struct DeviceCodeReply {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

/// One poll of the access-token endpoint.
#[derive(Debug, PartialEq, Eq)]
enum Poll {
    Token(String),
    Pending,
    SlowDown,
}

fn parse_poll(v: &Value) -> Result<Poll, AgentError> {
    if let Some(token) = v["access_token"].as_str() {
        return Ok(Poll::Token(token.to_owned()));
    }
    let fail = |m: &str| Err(AgentError::Provider(m.to_owned()));
    match v["error"].as_str() {
        Some("authorization_pending") => Ok(Poll::Pending),
        Some("slow_down") => Ok(Poll::SlowDown),
        Some("expired_token") => fail("the code expired before it was approved; sign in again"),
        Some("access_denied") => fail("the sign-in was denied on GitHub"),
        Some(other) => fail(v["error_description"].as_str().unwrap_or(other)),
        None => fail("GitHub answered the sign-in with something Krampsta did not expect"),
    }
}

/// Signs in with the device flow: asks GitHub for a code, hands it to `prompt`, then polls
/// until the user approves it. Resolves to the GitHub OAuth token. Cancel by dropping it.
pub async fn device_login(
    client: &reqwest::Client,
    github: &GitHub,
    prompt: LoginPrompt,
) -> Result<String, AgentError> {
    let reply = client
        .post(format!("{}/login/device/code", github.web))
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, UA)
        .form(&[("client_id", CLIENT_ID), ("scope", "read:user")])
        .timeout(LOOKUP_TIMEOUT)
        .send()
        .await
        .map_err(|e| AgentError::Provider(format!("could not reach GitHub: {e}")))?;
    let status = reply.status();
    let body = reply.text().await.map_err(|e| AgentError::Provider(e.to_string()))?;
    if !status.is_success() {
        return Err(http::status_error(status, &body).into());
    }
    let code: DeviceCodeReply = serde_json::from_str(&body)
        .map_err(|e| AgentError::Provider(format!("GitHub sent a device code Krampsta could not read ({e})")))?;
    prompt(DeviceCode { url: code.verification_uri.clone(), code: code.user_code.clone() });

    let mut interval = code.interval.max(1);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in.max(60));
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() > deadline {
            return Err(AgentError::Provider("the code expired before it was approved; sign in again".into()));
        }
        let reply = client
            .post(format!("{}/login/oauth/access_token", github.web))
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, UA)
            .form(&[
                ("client_id", CLIENT_ID),
                ("device_code", code.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .timeout(LOOKUP_TIMEOUT)
            .send()
            .await
            .map_err(|e| AgentError::Provider(format!("could not reach GitHub: {e}")))?;
        let body = reply.text().await.map_err(|e| AgentError::Provider(e.to_string()))?;
        let v: Value = serde_json::from_str(&body).map_err(|e| {
            AgentError::Provider(format!("GitHub answered the sign-in with something unreadable ({e})"))
        })?;
        match parse_poll(&v)? {
            Poll::Token(token) => return Ok(token),
            Poll::Pending => {}
            Poll::SlowDown => interval += 5,
        }
    }
}

/// The GitHub login the token belongs to ("octocat").
pub async fn user_login(client: &reqwest::Client, github: &GitHub, token: &str) -> Result<String, AgentError> {
    let req = client
        .get(format!("{}/user", github.api))
        .header(AUTHORIZATION, format!("token {token}"))
        .header(USER_AGENT, UA);
    let v = http::get_json(req, LOOKUP_TIMEOUT).await?;
    Ok(v["login"].as_str().unwrap_or("GitHub user").to_owned())
}

/// The short-lived token Copilot's API takes, and where that API lives for this account.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CopilotToken {
    token: String,
    /// Unix seconds.
    expires_at: i64,
    api: String,
}

fn parse_token(v: &Value) -> Result<CopilotToken, AgentError> {
    let token = v["token"].as_str().ok_or_else(|| AgentError::Provider("Copilot sent no token".into()))?;
    Ok(CopilotToken {
        token: token.to_owned(),
        expires_at: v["expires_at"].as_i64().unwrap_or(0),
        api: v["endpoints"]["api"].as_str().unwrap_or(DEFAULT_API).trim_end_matches('/').to_owned(),
    })
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Mints (and caches) Copilot tokens from the stored GitHub token.
pub struct CopilotAccess {
    client: reqwest::Client,
    github: GitHub,
    github_token: String,
    cached: Mutex<Option<CopilotToken>>,
}

impl CopilotAccess {
    pub fn new(client: reqwest::Client, github: GitHub, github_token: String) -> Self {
        CopilotAccess { client, github, github_token, cached: Mutex::new(None) }
    }

    async fn token(&self) -> Result<CopilotToken, AgentError> {
        let mut cached = self.cached.lock().await;
        if let Some(t) = cached.as_ref().filter(|t| t.expires_at - REFRESH_MARGIN > now()) {
            return Ok(t.clone());
        }
        let req = self
            .client
            .get(format!("{}/copilot_internal/v2/token", self.github.api))
            .header(AUTHORIZATION, format!("token {}", self.github_token))
            .header(USER_AGENT, UA)
            .header("editor-version", EDITOR_VERSION)
            .header("editor-plugin-version", PLUGIN_VERSION);
        let v = http::get_json(req, LOOKUP_TIMEOUT).await.map_err(|e| match e {
            http::HttpError::Unauthorized { .. } => AgentError::Provider(
                "GitHub no longer accepts this sign-in, or the account has no Copilot subscription; \
                 sign in again in Settings"
                    .into(),
            ),
            other => other.into(),
        })?;
        let token = parse_token(&v)?;
        *cached = Some(token.clone());
        Ok(token)
    }
}

impl Access for CopilotAccess {
    fn endpoint(&self) -> BoxFuture<'_, Result<Endpoint, AgentError>> {
        Box::pin(async move {
            let t = self.token().await?;
            Ok(Endpoint { headers: headers(&t.token)?, base_url: t.api })
        })
    }
}

/// The headers every Copilot API request carries. Each run starts from the user's message, so
/// it counts as a user request (`x-initiator: user`).
fn headers(token: &str) -> Result<HeaderMap, AgentError> {
    let mut h = HeaderMap::new();
    h.insert(AUTHORIZATION, bearer(token)?);
    for (name, value) in [
        ("editor-version", EDITOR_VERSION),
        ("editor-plugin-version", PLUGIN_VERSION),
        ("copilot-integration-id", "vscode-chat"),
        ("user-agent", UA),
        ("openai-intent", "conversation-edits"),
        ("x-initiator", "user"),
    ] {
        h.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
    }
    Ok(h)
}

/// The chat models this account may use through Chat Completions, in the server's order.
pub async fn list_models(client: &reqwest::Client, access: &CopilotAccess) -> Result<Vec<String>, AgentError> {
    let endpoint = access.endpoint().await?;
    let req = client.get(endpoint.url("models")).headers(endpoint.headers);
    Ok(parse_models(&http::get_json(req, LOOKUP_TIMEOUT).await?))
}

fn parse_models(v: &Value) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for m in v["data"].as_array().into_iter().flatten() {
        let Some(id) = m["id"].as_str() else { continue };
        if m["capabilities"]["type"] == "chat" && usable(m) && speaks_chat(m) && !ids.iter().any(|i| i == id) {
            ids.push(id.to_owned());
        }
    }
    ids
}

/// Some accounts get `model_picker_enabled: false` on every model; there the policy says which
/// ones the account may use. A model whose policy is disabled needs enabling on github.com.
fn usable(m: &Value) -> bool {
    match m["policy"]["state"].as_str() {
        Some("disabled") => false,
        Some("enabled") => true,
        _ => m["model_picker_enabled"].as_bool().unwrap_or(false),
    }
}

/// Models that list their endpoints must include Chat Completions (some only speak the
/// Responses API, which this engine doesn't use yet).
fn speaks_chat(m: &Value) -> bool {
    match m["supported_endpoints"].as_array() {
        Some(endpoints) => endpoints.iter().any(|e| e == "/chat/completions"),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn device_flow_answers() {
        assert_eq!(parse_poll(&json!({"access_token": "gho_x"})).unwrap(), Poll::Token("gho_x".into()));
        assert_eq!(parse_poll(&json!({"error": "authorization_pending"})).unwrap(), Poll::Pending);
        assert_eq!(parse_poll(&json!({"error": "slow_down"})).unwrap(), Poll::SlowDown);
        let expired = parse_poll(&json!({"error": "expired_token"})).unwrap_err();
        assert!(expired.to_string().contains("expired"), "{expired}");
    }

    #[test]
    fn the_token_exchange_reads_the_accounts_api_root() {
        let t = parse_token(&json!({"token": "tid=1", "expires_at": 1_762_480_000, "endpoints": {"api": "https://api.individual.githubcopilot.com/"}})).unwrap();
        assert_eq!(t.api, "https://api.individual.githubcopilot.com");
        assert_eq!(t.expires_at, 1_762_480_000);
        assert_eq!(parse_token(&json!({"token": "t"})).unwrap().api, DEFAULT_API);
        assert!(parse_token(&json!({})).is_err());
    }

    #[test]
    fn requests_carry_copilots_headers() {
        let h = headers("tok").unwrap();
        assert_eq!(h["authorization"], "Bearer tok");
        assert_eq!(h["copilot-integration-id"], "vscode-chat");
        assert_eq!(h["x-initiator"], "user");
    }

    #[test]
    fn only_usable_chat_completions_models_are_listed() {
        let v = json!({"data": [
            {"id": "gpt-4.1", "capabilities": {"type": "chat"}, "model_picker_enabled": true},
            {"id": "claude-sonnet-4.5", "capabilities": {"type": "chat"}, "policy": {"state": "enabled"},
             "supported_endpoints": ["/chat/completions"]},
            {"id": "gpt-5-codex", "capabilities": {"type": "chat"}, "model_picker_enabled": true,
             "supported_endpoints": ["/responses"]},
            {"id": "o3", "capabilities": {"type": "chat"}, "policy": {"state": "disabled"}, "model_picker_enabled": true},
            {"id": "text-embedding-3-small", "capabilities": {"type": "embeddings"}, "model_picker_enabled": true},
            {"id": "gpt-4.1", "capabilities": {"type": "chat"}, "model_picker_enabled": true}
        ]});
        assert_eq!(parse_models(&v), ["gpt-4.1", "claude-sonnet-4.5"]);
    }
}
