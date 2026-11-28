//! `fetch_url`: a web page (http or https) for the model to read: HTML as plain text, other text
//! as it comes, binaries refused, at most [`MAX_TEXT`] of it. Each site asks the user unless a
//! rule allows it (`fetch_url:<host>`, [`rule`]); a redirect to another site stops, so that one
//! asks too. What the page says is data for the model, never instructions (see the run's
//! instructions).

use std::io::Read;
use std::time::Duration;

use krampsta_agent::CancellationToken;
use reqwest::Url;
use serde::Deserialize;

pub const TOOL: &str = "fetch_url";
/// The most of the page's text the model gets; the rest is said to be left out.
pub const MAX_TEXT: usize = 32 * 1024;
/// The most of a reply downloaded, before turning it into text.
const MAX_DOWNLOAD: u64 = 2 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
/// Redirects followed within one site.
const MAX_REDIRECTS: usize = 5;
/// The width HTML is laid out to as text.
const WIDTH: usize = 100;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub url: String,
}

/// The address, if it is one this tool fetches, and its host.
pub fn parse_url(url: &str) -> Result<(Url, String), String> {
    let parsed = Url::parse(url.trim()).map_err(|e| format!("`{url}` is not a URL ({e})"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("only http and https pages can be fetched, not `{}:`", parsed.scheme()));
    }
    let host = parsed.host_str().filter(|h| !h.is_empty()).ok_or_else(|| format!("`{url}` names no host"))?;
    let host = host.to_ascii_lowercase();
    Ok((parsed, host))
}

/// The rule that allows fetching from `host` without asking.
pub fn rule(host: &str) -> String {
    format!("{TOOL}:{host}")
}

/// Fetches the page and returns its text, headed by where it ended up.
pub fn run(args: &Args, cancel: &CancellationToken) -> Result<String, String> {
    let (url, host) = parse_url(&args.url)?;
    let same_site = host.clone();
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("krampsta/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            let next = attempt.url().host_str().map(str::to_ascii_lowercase);
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if next.as_deref() == Some(same_site.as_str()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|e| format!("the HTTP client could not start: {e}"))?;
    let response = client.get(url.clone()).send().map_err(|e| readable(&url, &e))?;
    if cancel.is_cancelled() {
        return Err("Stopped.".into());
    }
    let status = response.status();
    if status.is_redirection() {
        let to = response.headers().get("location").and_then(|l| l.to_str().ok()).unwrap_or("another address");
        return Err(format!("{url} redirects to another site ({to}); fetch that URL if you need it"));
    }
    if !status.is_success() {
        return Err(format!("{url} answered HTTP {status}"));
    }
    let landed = response.url().clone();
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|t| t.to_str().ok())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let mut bytes = Vec::new();
    response.take(MAX_DOWNLOAD + 1).read_to_end(&mut bytes).map_err(|e| format!("the page stopped arriving: {e}"))?;
    if cancel.is_cancelled() {
        return Err("Stopped.".into());
    }
    let cut_download = bytes.len() as u64 > MAX_DOWNLOAD;
    bytes.truncate(MAX_DOWNLOAD as usize);
    let text = to_text(&kind, &bytes)?;
    Ok(present(&landed, &text, cut_download))
}

/// The reply as text: HTML laid out as plain text, other text types as they are.
fn to_text(kind: &str, bytes: &[u8]) -> Result<String, String> {
    let html = kind.contains("html") || (kind.is_empty() && looks_like_html(bytes));
    if html {
        return html2text::from_read(bytes, WIDTH).map_err(|e| format!("the page's HTML could not be read: {e}"));
    }
    const TEXTUAL: [&str; 6] = ["text/", "json", "xml", "javascript", "yaml", "toml"];
    if TEXTUAL.iter().any(|t| kind.contains(t)) || (kind.is_empty() && std::str::from_utf8(bytes).is_ok()) {
        return Ok(String::from_utf8_lossy(bytes).into_owned());
    }
    Err(format!("the page is not text ({kind}); only text and HTML can be read"))
}

fn looks_like_html(bytes: &[u8]) -> bool {
    let start = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    start.contains("<html") || start.contains("<!doctype html")
}

/// The text for the model: where it came from, at most [`MAX_TEXT`] of it, and what was left out.
fn present(url: &Url, text: &str, cut_download: bool) -> String {
    let text = text.trim();
    let mut cut = text.len().min(MAX_TEXT);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = format!("URL: {url}\n\n{}", &text[..cut]);
    if cut < text.len() {
        out.push_str(&format!("\n\n[{} more bytes of text left out]", text.len() - cut));
    } else if cut_download {
        out.push_str(&format!("\n\n[the page is longer than {} MB; the rest was not downloaded]", MAX_DOWNLOAD >> 20));
    }
    out
}

/// A request error in words: the site can't be reached, it took too long, or something else.
fn readable(url: &Url, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        format!("{url} took longer than {} s to answer", TIMEOUT.as_secs())
    } else if e.is_connect() {
        format!("{url} could not be reached")
    } else {
        format!("{url} could not be fetched: {e}")
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::TcpListener;

    use super::*;

    /// Answers one request with `status`, `headers` and `body`; gives back the URL.
    fn serve(status: &'static str, headers: &'static str, body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/page", listener.local_addr().unwrap().port());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let head =
                format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        });
        url
    }

    fn fetch(url: String) -> Result<String, String> {
        run(&Args { url }, &CancellationToken::new())
    }

    #[test]
    fn html_comes_back_as_text_without_scripts() {
        let page = b"<!doctype html><html><head><title>T</title><script>var secret = 1;</script></head>\
            <body><h1>Retry policy</h1><p>Waits <b>double</b> each time.</p></body></html>";
        let url = serve("200 OK", "Content-Type: text/html; charset=utf-8\r\n", page.to_vec());
        let text = fetch(url.clone()).unwrap();
        assert!(text.starts_with(&format!("URL: {url}")), "{text}");
        assert!(text.contains("Retry policy") && text.contains("double"), "{text}");
        assert!(!text.contains("var secret") && !text.contains("<p>"), "{text}");
    }

    #[test]
    fn plain_text_is_kept_and_long_text_is_cut() {
        let url = serve("200 OK", "Content-Type: application/json\r\n", br#"{"ok": true}"#.to_vec());
        assert!(fetch(url).unwrap().ends_with(r#"{"ok": true}"#));
        let url = serve("200 OK", "Content-Type: text/plain\r\n", vec![b'x'; MAX_TEXT + 100]);
        let text = fetch(url).unwrap();
        assert!(text.ends_with("[100 more bytes of text left out]"), "{}", &text[text.len() - 60..]);
    }

    #[test]
    fn binaries_errors_and_other_schemes_are_refused_readably() {
        let url = serve("200 OK", "Content-Type: image/png\r\n", vec![0x89, b'P', b'N', b'G']);
        assert!(fetch(url).unwrap_err().contains("not text (image/png)"));
        let url = serve("404 Not Found", "", b"nope".to_vec());
        assert!(fetch(url).unwrap_err().ends_with("answered HTTP 404 Not Found"));
        assert!(fetch("file:///etc/passwd".into()).unwrap_err().contains("only http and https"));
        assert!(fetch("not a url".into()).unwrap_err().contains("is not a URL"));
    }

    #[test]
    fn a_redirect_to_another_site_stops_there() {
        let url = serve("302 Found", "Location: https://elsewhere.example/x\r\n", Vec::new());
        let error = fetch(url).unwrap_err();
        assert!(error.contains("redirects to another site (https://elsewhere.example/x)"), "{error}");
    }

    #[test]
    fn rules_name_the_host() {
        let (_, host) = parse_url("https://Docs.RS/serde/latest").unwrap();
        assert_eq!(rule(&host), "fetch_url:docs.rs");
    }
}
