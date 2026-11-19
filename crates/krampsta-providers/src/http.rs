//! HTTP plumbing shared by the engines: one `reqwest` client, a cancellable SSE request, and
//! errors worded for the person reading the run's error card.

use std::time::Duration;

use futures_util::StreamExt;
use krampsta_agent::{AgentError, RunContext};
use reqwest::StatusCode;

use crate::sse::{SseEvent, SseParser};

/// How long to wait for a connection before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The longest silence allowed between two pieces of a streamed reply.
const READ_TIMEOUT: Duration = Duration::from_secs(120);
/// How much of an error body is kept for the message when it isn't JSON.
const DETAIL_CHARS: usize = 300;

/// Why a provider request failed.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum HttpError {
    #[error("the provider rejected the credentials (HTTP {status}): {detail}")]
    Unauthorized { status: u16, detail: String },
    #[error("the provider's rate limit or quota was reached (HTTP 429): {detail}")]
    RateLimited { detail: String },
    #[error("the provider answered HTTP {status}: {detail}")]
    Status { status: u16, detail: String },
    #[error("could not reach {url}: {reason}")]
    Connect { url: String, reason: String },
    #[error("the provider stopped responding (no data for {} s)", READ_TIMEOUT.as_secs())]
    Timeout,
    #[error("the connection broke while streaming: {0}")]
    Stream(String),
    #[error("the HTTP client could not start: {0}")]
    Client(String),
}

impl From<HttpError> for AgentError {
    fn from(e: HttpError) -> Self {
        AgentError::Provider(e.to_string())
    }
}

/// What an SSE handler wants after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// The stream said it is finished (`data: [DONE]`); stop reading.
    Done,
}

/// The client every engine request goes through (connection pool, timeouts, user agent).
pub fn client() -> Result<reqwest::Client, HttpError> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .user_agent(concat!("krampsta/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| HttpError::Client(chain(&e)))
}

/// Sends `request`, then feeds each SSE event of the reply to `on_event` until the body ends,
/// the handler returns [`Flow::Done`], or the run is cancelled. Cancelling drops the response,
/// which closes the connection: the provider stops generating.
pub async fn stream_sse(
    request: reqwest::RequestBuilder,
    ctx: &RunContext,
    on_event: impl FnMut(SseEvent) -> Result<Flow, AgentError>,
) -> Result<(), AgentError> {
    let mut tape = record::folder().map(|dir| (dir, Vec::new()));
    let result = stream_into(request, ctx, on_event, tape.as_mut().map(|(_, bytes)| bytes)).await;
    if let Some((dir, bytes)) = tape {
        record::save(dir, bytes).await;
    }
    result
}

async fn stream_into(
    request: reqwest::RequestBuilder,
    ctx: &RunContext,
    mut on_event: impl FnMut(SseEvent) -> Result<Flow, AgentError>,
    mut tape: Option<&mut Vec<u8>>,
) -> Result<(), AgentError> {
    let response =
        cancellable(ctx, request.header("accept", "text/event-stream").send()).await?.map_err(|e| request_error(&e))?;
    let status = response.status();
    if !status.is_success() {
        let body = cancellable(ctx, response.text()).await?.unwrap_or_default();
        return Err(status_error(status, &body).into());
    }

    let mut parser = SseParser::new();
    let mut body = response.bytes_stream();
    while let Some(chunk) = cancellable(ctx, body.next()).await? {
        let chunk = chunk.map_err(|e| request_error(&e))?;
        if let Some(tape) = tape.as_deref_mut() {
            tape.extend_from_slice(&chunk);
        }
        for event in parser.push(&chunk) {
            ctx.check_cancelled()?;
            if on_event(event)? == Flow::Done {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// `KRAMPSTA_RECORD_SSE=<folder>`: a development mode that saves every streamed reply byte for
/// byte, one `.sse` file per request, to turn into test fixtures. The files hold the whole
/// conversation (and encrypted reasoning): it is never on unless asked for, and a recording is
/// reviewed before it is committed.
mod record {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub fn folder() -> Option<PathBuf> {
        std::env::var_os("KRAMPSTA_RECORD_SSE").filter(|v| !v.is_empty()).map(PathBuf::from)
    }

    /// Writes one recording off the async workers. A failure is reported, not fatal: the run
    /// already has its answer.
    pub async fn save(dir: PathBuf, bytes: Vec<u8>) {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        if bytes.is_empty() {
            return;
        }
        let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let path = dir.join(format!("{millis}-{}.sse", COUNT.fetch_add(1, Ordering::Relaxed)));
        let job = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes)).map(|()| path)
        });
        match job.await {
            Ok(Ok(path)) => eprintln!("krampsta: recorded a stream to {}", path.display()),
            Ok(Err(e)) => eprintln!("krampsta: could not record the stream: {e}"),
            Err(e) => eprintln!("krampsta: recording the stream stopped abnormally: {e}"),
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn a_recording_is_saved_byte_for_byte_and_an_empty_one_is_skipped() {
            let dir = std::env::temp_dir().join(format!("krampsta-record-{}", std::process::id()));
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let bytes = b"data: {\"x\":\"\xc3\xa9\"}\r\n\r\ndata: [DONE]\n\n".to_vec();
            rt.block_on(super::save(dir.clone(), bytes.clone()));
            rt.block_on(super::save(dir.clone(), Vec::new()));
            let files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
            assert_eq!(files.len(), 1, "{files:?}");
            assert_eq!(std::fs::read(&files[0]).unwrap(), bytes);
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}

/// Runs `fut` unless the run is cancelled first.
async fn cancellable<T>(ctx: &RunContext, fut: impl Future<Output = T>) -> Result<T, AgentError> {
    tokio::select! {
        biased;
        () = ctx.cancellation().cancelled() => Err(AgentError::Cancelled),
        out = fut => Ok(out),
    }
}

/// Sends `request` and parses the JSON reply, failing after `timeout` (lookups like a model
/// list should not hang a picker when a local server isn't running).
pub async fn get_json(request: reqwest::RequestBuilder, timeout: Duration) -> Result<serde_json::Value, HttpError> {
    let response = request.timeout(timeout).send().await.map_err(|e| request_error(&e))?;
    let status = response.status();
    let body = response.text().await.map_err(|e| request_error(&e))?;
    if !status.is_success() {
        return Err(status_error(status, &body));
    }
    serde_json::from_str(&body).map_err(|e| HttpError::Stream(format!("the reply was not valid JSON ({e})")))
}

fn request_error(e: &reqwest::Error) -> HttpError {
    if e.is_timeout() {
        HttpError::Timeout
    } else if e.is_connect() {
        let url = e.url().map_or_else(|| "the provider".to_owned(), |u| u.as_str().to_owned());
        HttpError::Connect { url, reason: chain(e) }
    } else if e.is_body() || e.is_decode() {
        HttpError::Stream(chain(e))
    } else {
        HttpError::Client(chain(e))
    }
}

/// Maps a non-2xx reply to an error, keeping the provider's own explanation.
pub fn status_error(status: StatusCode, body: &str) -> HttpError {
    let detail = error_detail(body).unwrap_or_else(|| status.canonical_reason().unwrap_or("no details").to_owned());
    match status.as_u16() {
        401 | 403 => HttpError::Unauthorized { status: status.as_u16(), detail },
        429 => HttpError::RateLimited { detail },
        code => HttpError::Status { status: code, detail },
    }
}

/// The human part of an error body: `error.message` (OpenAI and most compatibles), `message`
/// or `detail`, else the start of the raw text.
fn error_detail(body: &str) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        let found = [&json["error"]["message"], &json["error"], &json["message"], &json["detail"]]
            .into_iter()
            .find_map(|v| v.as_str().filter(|s| !s.is_empty()));
        if let Some(text) = found {
            return Some(text.to_owned());
        }
    }
    let mut short: String = body.chars().take(DETAIL_CHARS).collect();
    if short.len() < body.len() {
        short.push('…');
    }
    Some(short)
}

/// An error and its causes on one line ("error sending request: tcp connect error: refused").
fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_errors_keep_the_providers_message() {
        let e = status_error(StatusCode::UNAUTHORIZED, r#"{"error":{"message":"Incorrect API key provided"}}"#);
        assert_eq!(e, HttpError::Unauthorized { status: 401, detail: "Incorrect API key provided".into() });
        assert!(e.to_string().contains("rejected the credentials"));
    }

    #[test]
    fn rate_limits_and_other_statuses_are_told_apart() {
        assert!(matches!(status_error(StatusCode::TOO_MANY_REQUESTS, "{}"), HttpError::RateLimited { .. }));
        let e = status_error(StatusCode::NOT_FOUND, r#"{"detail":"model not found"}"#);
        assert_eq!(e, HttpError::Status { status: 404, detail: "model not found".into() });
    }

    #[test]
    fn non_json_bodies_are_shortened_and_empty_ones_use_the_reason() {
        let long = "x".repeat(1000);
        let HttpError::Status { detail, .. } = status_error(StatusCode::BAD_GATEWAY, &long) else { panic!() };
        assert_eq!(detail.chars().count(), DETAIL_CHARS + 1);
        let HttpError::Status { detail, .. } = status_error(StatusCode::BAD_GATEWAY, "") else { panic!() };
        assert_eq!(detail, "Bad Gateway");
    }
}
