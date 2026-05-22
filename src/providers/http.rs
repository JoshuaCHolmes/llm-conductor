//! Shared HTTP client + retry helpers used by every provider.
//!
//! Goal: one place that decides how long we wait, when we retry, and how we
//! sanitize error bodies. This keeps providers focused on protocol shape and
//! leaves transport robustness here.

use anyhow::Result;
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use std::time::Duration;

/// Connect/dial timeout — keep tight; the network either answers fast or it doesn't.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Whole-request timeout for non-streaming chat/JSON APIs.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Whole-request timeout when the response is a long-lived stream.
/// reqwest's per-request timeout fires even mid-stream, so streaming providers
/// should pass `None` (no overall timeout) and rely on per-chunk timeouts in
/// their own loop.
pub const NO_TIMEOUT: Option<Duration> = None;

/// Maximum time to wait for the first chunk of an SSE/NDJSON stream after the
/// HTTP response headers arrive. Larger than per-chunk because cold model loads
/// (e.g. NVIDIA NIM, Ollama swapping models) can be slow.
pub const FIRST_CHUNK_TIMEOUT: Duration = Duration::from_secs(60);
/// Maximum silence between subsequent stream chunks. If exceeded, treat the
/// stream as stalled and surface a timeout error rather than hanging the agent.
pub const CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
/// Default timeout for non-streaming `chat()` calls when the provider's HTTP
/// client itself has no overall timeout (because it's shared with streaming).
pub const CHAT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Build a `reqwest::Client` with sensible defaults for chat APIs.
///
/// Pass `None` for `request_timeout` when the client will be used for
/// streaming responses; pass `Some(_)` for plain JSON request/response APIs.
pub fn build_client(request_timeout: Option<Duration>) -> Result<Client> {
    let mut b = Client::builder().connect_timeout(DEFAULT_CONNECT_TIMEOUT);
    if let Some(t) = request_timeout {
        b = b.timeout(t);
    }
    Ok(b.build()?)
}

/// Send a request, retrying on 429 (with `Retry-After` honoured), 5xx, and
/// transient connect/timeout errors.
///
/// `make_req` is a closure rather than a `RequestBuilder` because
/// `RequestBuilder::try_clone` returns `None` for streaming bodies and we want
/// retries to be reliable. Each invocation should rebuild the request from
/// shared inputs the caller closes over.
pub async fn send_with_retry<F>(make_req: F, max_retries: u32) -> Result<Response>
where
    F: Fn() -> RequestBuilder,
{
    let mut attempt: u32 = 0;
    loop {
        let req = make_req();
        match req.send().await {
            Ok(resp) => {
                let s = resp.status();
                if (s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error())
                    && attempt < max_retries
                {
                    let wait = if s == StatusCode::TOO_MANY_REQUESTS {
                        parse_retry_after(&resp).unwrap_or_else(|| backoff(attempt))
                    } else {
                        backoff(attempt)
                    };
                    tracing::warn!(
                        "HTTP {} on attempt {}/{}; retrying after {:?}",
                        s,
                        attempt + 1,
                        max_retries + 1,
                        wait
                    );
                    drop(resp);
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                    continue;
                }
                return Ok(resp);
            }
            Err(e) if (e.is_connect() || e.is_timeout()) && attempt < max_retries => {
                let wait = backoff(attempt);
                tracing::warn!(
                    "transport error on attempt {}/{}: {}; retrying after {:?}",
                    attempt + 1,
                    max_retries + 1,
                    e,
                    wait
                );
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn backoff(attempt: u32) -> Duration {
    // 250ms, 500ms, 1s, 2s, 4s, 8s — capped.
    let ms = 250u64.saturating_mul(1u64 << attempt.min(5));
    Duration::from_millis(ms)
}

fn parse_retry_after(resp: &Response) -> Option<Duration> {
    let v = resp.headers().get("retry-after")?.to_str().ok()?;
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs.min(60)));
    }
    // HTTP-date form is rare from chat APIs; ignore and let backoff() handle it.
    None
}

/// Sanitize an HTTP response body for inclusion in an error message.
///
/// - Truncates to ~500 chars (server stack traces aren't useful at scale).
/// - Replaces values of common credential-bearing fields with `[REDACTED]`.
/// - Trims very long opaque tokens that look like cookies/jwts.
pub fn sanitize_error_body(body: &str) -> String {
    const MAX: usize = 500;
    let total_chars = body.chars().count();
    let truncated: String = if total_chars > MAX {
        let head: String = body.chars().take(MAX).collect();
        format!("{head}…[truncated {} more chars]", total_chars - MAX)
    } else {
        body.to_string()
    };

    let redacted = redact_sensitive_fields(&truncated);
    mask_long_tokens(&redacted)
}

/// Replace values of `"<sensitive_key>": "<value>"` (case-insensitive key match)
/// with `"<sensitive_key>": "[REDACTED]"`. Operates on `&str`, returns owned `String`.
fn redact_sensitive_fields(input: &str) -> String {
    const KEYS: &[&str] = &[
        "cookie",
        "csrf",
        "csrftoken",
        "csrf_token",
        "x-csrf-token",
        "session",
        "sessionid",
        "session_id",
        "authorization",
        "bearer",
        "apikey",
        "api_key",
        "api-key",
        "password",
        "secret",
        "token",
        "accesstoken",
        "access_token",
    ];

    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    let bytes = input.as_bytes();

    while cursor < bytes.len() {
        // Look for the next quote — start of a possible JSON key.
        let next_q = match input[cursor..].find('"') {
            Some(i) => cursor + i,
            None => {
                out.push_str(&input[cursor..]);
                break;
            }
        };
        out.push_str(&input[cursor..next_q]);

        // Find the matching closing quote of the key (no escape support — keys are simple).
        let after_open = next_q + 1;
        let close_q = match input[after_open..].find('"') {
            Some(i) => after_open + i,
            None => {
                out.push_str(&input[next_q..]);
                break;
            }
        };
        let key = &input[after_open..close_q];

        // Determine if this is a sensitive key followed by `:"value"`.
        let after_close = close_q + 1;
        let rest = &input[after_close..];
        let trimmed = rest.trim_start();
        let is_sensitive = KEYS.iter().any(|k| k.eq_ignore_ascii_case(key));

        if is_sensitive && trimmed.starts_with(':') && trimmed[1..].trim_start().starts_with('"') {
            // Locate the value's opening and closing quotes (with simple `\"` escape support).
            let value_open_offset_in_rest = {
                let colon_idx = rest.find(':').unwrap();
                let after_colon = &rest[colon_idx + 1..];
                let leading_ws = after_colon.len() - after_colon.trim_start().len();
                colon_idx + 1 + leading_ws
            };
            let value_open_abs = after_close + value_open_offset_in_rest;
            let value_start = value_open_abs + 1;
            let mut value_end = None;
            let value_bytes = &input.as_bytes()[value_start..];
            let mut prev_bs = false;
            for (i, &b) in value_bytes.iter().enumerate() {
                if b == b'"' && !prev_bs {
                    value_end = Some(value_start + i);
                    break;
                }
                prev_bs = b == b'\\' && !prev_bs;
            }
            if let Some(end) = value_end {
                out.push_str(&format!("\"{}\":\"[REDACTED]\"", key));
                cursor = end + 1;
                continue;
            }
        }

        // Not sensitive (or malformed) — copy the key verbatim and resume after the closing quote.
        out.push_str(&input[next_q..=close_q]);
        cursor = close_q + 1;
    }

    out
}

fn mask_long_tokens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric()
            || c == '-'
            || c == '_'
            || c == '.'
            || c == '='
            || c == '+'
            || c == '/'
        {
            buf.push(c);
        } else {
            if buf.len() >= 64 {
                out.push_str("[REDACTED]");
            } else {
                out.push_str(&buf);
            }
            buf.clear();
            out.push(c);
        }
    }
    if buf.len() >= 64 {
        out.push_str("[REDACTED]");
    } else {
        out.push_str(&buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_bodies() {
        let body = "x".repeat(2000);
        let s = sanitize_error_body(&body);
        assert!(s.contains("[truncated"));
        assert!(s.len() < body.len());
    }

    #[test]
    fn redacts_cookie_field() {
        let body = r#"{"error":"bad","cookie":"session=abc; csrf=def"}"#;
        let s = sanitize_error_body(body);
        assert!(s.contains("\"cookie\":\"[REDACTED]\""), "got: {}", s);
        assert!(!s.contains("session=abc"));
    }

    #[test]
    fn redacts_csrf_token_field() {
        let body = r#"{"x-csrf-token":"deadbeef-1234-5678","ok":false}"#;
        let s = sanitize_error_body(body);
        assert!(s.contains("\"x-csrf-token\":\"[REDACTED]\""), "got: {}", s);
    }

    #[test]
    fn masks_long_opaque_tokens() {
        let body =
            "set-cookie: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=value";
        let s = sanitize_error_body(body);
        assert!(s.contains("[REDACTED]"));
    }

    #[test]
    fn short_strings_pass_through() {
        let body = r#"{"error":"not found"}"#;
        let s = sanitize_error_body(body);
        assert_eq!(s, body);
    }

    // ===== send_with_retry behavioral tests (wiremock) =====

    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mk_client() -> Client {
        build_client(Some(Duration::from_secs(2))).unwrap()
    }

    #[tokio::test]
    async fn retry_happy_path_no_retries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;
        let url = format!("{}/x", server.uri());
        let client = mk_client();
        let resp = send_with_retry(|| client.post(&url), 3).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn retry_on_5xx_then_succeeds() {
        let server = MockServer::start().await;
        // First call: 503; subsequent: 200.
        Mock::given(method("POST"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let url = format!("{}/x", server.uri());
        let client = mk_client();
        let resp = send_with_retry(|| client.post(&url), 3).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn retry_on_429_with_retry_after() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let url = format!("{}/x", server.uri());
        let client = mk_client();
        let started = std::time::Instant::now();
        let resp = send_with_retry(|| client.post(&url), 3).await.unwrap();
        assert_eq!(resp.status(), 200);
        // Honored retry-after >= ~1s.
        assert!(started.elapsed() >= Duration::from_millis(900));
    }

    #[tokio::test]
    async fn retry_exhausts_and_returns_last_error_status() {
        let server = MockServer::start().await;
        let counter = Arc::new(AtomicU32::new(0));
        let c2 = counter.clone();
        Mock::given(method("POST"))
            .and(path("/x"))
            .respond_with(move |_: &wiremock::Request| {
                c2.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(500)
            })
            .mount(&server)
            .await;
        let url = format!("{}/x", server.uri());
        let client = mk_client();
        let resp = send_with_retry(|| client.post(&url), 2).await.unwrap();
        // After max_retries=2, third response is returned even if 5xx.
        assert_eq!(resp.status(), 500);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_passes_through_4xx_without_retry() {
        let server = MockServer::start().await;
        let counter = Arc::new(AtomicU32::new(0));
        let c2 = counter.clone();
        Mock::given(method("POST"))
            .and(path("/x"))
            .respond_with(move |_: &wiremock::Request| {
                c2.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(404)
            })
            .mount(&server)
            .await;
        let url = format!("{}/x", server.uri());
        let client = mk_client();
        let resp = send_with_retry(|| client.post(&url), 5).await.unwrap();
        assert_eq!(resp.status(), 404);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
