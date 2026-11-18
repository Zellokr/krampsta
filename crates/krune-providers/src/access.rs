//! How an engine reaches its server: the API root and the headers (credentials included) for
//! the next request. A fixed key is the simple case; account-based providers (Copilot, a
//! ChatGPT plan) mint short-lived tokens and may move the API root, so they answer per request.

use krune_agent::{AgentError, BoxFuture};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

/// Where the next request goes and what it carries.
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// The API root, without a trailing slash (`https://api.openai.com/v1`).
    pub base_url: String,
    pub headers: HeaderMap,
}

impl Endpoint {
    pub fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url.trim_end_matches('/'))
    }
}

/// Answers [`Endpoint`]s, refreshing credentials as needed.
pub trait Access: Send + Sync + 'static {
    fn endpoint(&self) -> BoxFuture<'_, Result<Endpoint, AgentError>>;
}

/// A fixed API root and an optional API key.
pub struct StaticAccess {
    base_url: String,
    headers: HeaderMap,
}

impl StaticAccess {
    pub fn new(base_url: &str, api_key: Option<&str>) -> Result<Self, AgentError> {
        let mut headers = HeaderMap::new();
        if let Some(key) = api_key {
            headers.insert(AUTHORIZATION, bearer(key)?);
        }
        Ok(StaticAccess { base_url: base_url.trim_end_matches('/').to_owned(), headers })
    }
}

impl Access for StaticAccess {
    fn endpoint(&self) -> BoxFuture<'_, Result<Endpoint, AgentError>> {
        let endpoint = Endpoint { base_url: self.base_url.clone(), headers: self.headers.clone() };
        Box::pin(async move { Ok(endpoint) })
    }
}

/// `Authorization: Bearer <token>`, marked sensitive so it never shows in debug output.
pub fn bearer(token: &str) -> Result<HeaderValue, AgentError> {
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| AgentError::Provider("the credential contains characters a header can't carry".into()))?;
    value.set_sensitive(true);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_access_sends_the_key_only_when_there_is_one() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let with = rt.block_on(StaticAccess::new("http://h/v1/", Some("sk")).unwrap().endpoint()).unwrap();
        assert_eq!(with.url("models"), "http://h/v1/models");
        assert_eq!(with.headers[AUTHORIZATION], "Bearer sk");
        assert!(!format!("{:?}", with.headers).contains("sk"), "the key never prints");
        let without = rt.block_on(StaticAccess::new("http://h/v1", None).unwrap().endpoint()).unwrap();
        assert!(without.headers.is_empty());
    }
}
