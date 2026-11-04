//! The HTTP core: base URL, session token, JSON helpers, error mapping and
//! rate-limit handling.

use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use reqwest::{Method, StatusCode, header};
use serde::{Serialize, de::DeserializeOwned};
use url::Url;

use crate::ApiError;

/// Tries per request when the server answers 429, including the first.
pub const MAX_TRIES: u32 = 3;
/// The longest Matty waits before retrying a rate-limited request.
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an upload may take in all; the usual timeout is for requests
/// that send little.
pub(crate) const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Redirects followed within the server's own host, at most.
const MAX_REDIRECTS: usize = 5;

/// Waits between retries. Swapped for a fake in tests.
pub trait Sleeper: Send + Sync + 'static {
    /// Resolves after `duration`.
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// Sleeps on the tokio timer.
#[derive(Debug, Default)]
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(tokio::time::sleep(duration))
    }
}

/// The last `X-RateLimit-*` values the server sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateLimit {
    /// Requests per second the server allows.
    pub limit: Option<u32>,
    /// Requests left in the current window.
    pub remaining: Option<u32>,
    /// Seconds until the window resets.
    pub reset: Option<u64>,
}

impl RateLimit {
    fn from_headers(headers: &header::HeaderMap) -> Option<Self> {
        let number = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
        };
        let limit = number("x-ratelimit-limit");
        let remaining = number("x-ratelimit-remaining");
        let reset = number("x-ratelimit-reset");
        (limit.is_some() || remaining.is_some() || reset.is_some()).then(|| RateLimit {
            limit: limit.and_then(|n| u32::try_from(n).ok()),
            remaining: remaining.and_then(|n| u32::try_from(n).ok()),
            reset,
        })
    }
}

/// A session token. Its `Debug` never shows the value.
#[derive(Clone)]
struct Token(String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token([redacted])")
    }
}

/// Configures a [`Client`].
pub struct ClientBuilder {
    base: String,
    token: Option<String>,
    sleeper: Arc<dyn Sleeper>,
    timeout: Duration,
}

impl ClientBuilder {
    /// Uses `token` as the session token.
    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Replaces how the client waits between retries.
    pub fn sleeper(mut self, sleeper: impl Sleeper) -> Self {
        self.sleeper = Arc::new(sleeper);
        self
    }

    /// Overrides the per-request timeout (30 seconds by default).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Builds the client. The URL may omit the scheme, in which case
    /// `https://` is assumed (PLAN §4.5).
    pub fn build(self) -> Result<Client, ApiError> {
        let base = normalize_base(&self.base)?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("Matty/", env!("CARGO_PKG_VERSION")))
            .timeout(self.timeout)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(same_host_redirects())
            .tls_backend_preconfigured(crate::tls::client_config(crate::tls::HTTP_ALPN)?)
            .build()
            .map_err(ApiError::from_transport)?;
        Ok(Client {
            http,
            base,
            token: self.token.map(Token),
            sleeper: self.sleeper,
            rate_limit: Arc::new(Mutex::new(RateLimit::default())),
        })
    }
}

/// Follows a redirect only to the host (and port) it came from: the session
/// token and requests stay on the server. Another host's redirect comes back
/// as the 3xx response, which [`Client::send`] turns into
/// [`ApiError::Redirected`].
fn same_host_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let same = attempt.previous().first().is_some_and(|first| {
            first.host_str() == attempt.url().host_str()
                && first.port_or_known_default() == attempt.url().port_or_known_default()
        });
        if same && attempt.previous().len() <= MAX_REDIRECTS {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

fn normalize_base(input: &str) -> Result<Url, ApiError> {
    let trimmed = input.trim();
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let mut url = Url::parse(&with_scheme).map_err(|e| ApiError::InvalidUrl(e.to_string()))?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err(ApiError::InvalidUrl(format!(
            "unsupported scheme `{}`",
            url.scheme()
        )));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(ApiError::InvalidUrl("the URL has no host".into()));
    }
    // A server may live under a subpath; keep it, with a trailing slash so
    // joins land inside it.
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

/// The answer to a conditional bytes request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetched {
    /// The cached copy is still current (HTTP 304).
    NotModified,
    /// New content.
    Fresh {
        /// The body.
        bytes: Vec<u8>,
        /// The `ETag` to send next time.
        etag: Option<String>,
        /// The `Content-Type`.
        content_type: Option<String>,
    },
}

/// The largest outside image [`Client::outside_image`] takes, in bytes.
pub const MAX_OUTSIDE_IMAGE: u64 = 20 * 1024 * 1024;

/// A Mattermost REST v4 client for one server and, optionally, one session.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
    token: Option<Token>,
    sleeper: Arc<dyn Sleeper>,
    rate_limit: Arc<Mutex<RateLimit>>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base.as_str())
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Starts configuring a client for the server at `base_url`.
    pub fn builder(base_url: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            base: base_url.into(),
            token: None,
            sleeper: Arc::new(TokioSleeper),
            timeout: TIMEOUT,
        }
    }

    /// A client for `base_url` with default settings and no session.
    pub fn new(base_url: impl Into<String>) -> Result<Self, ApiError> {
        Self::builder(base_url).build()
    }

    /// The same client, signed in with `token`.
    pub fn with_token(&self, token: impl Into<String>) -> Self {
        Self {
            token: Some(Token(token.into())),
            ..self.clone()
        }
    }

    /// The server's base URL, always ending in `/`.
    pub fn base_url(&self) -> &Url {
        &self.base
    }

    /// The session token, for the WebSocket's authentication challenge and
    /// for keeping the session in the system's credential store.
    pub fn token_value(&self) -> Option<&str> {
        self.token.as_ref().map(|t| t.0.as_str())
    }

    /// Whether the client carries a session token.
    pub fn is_signed_in(&self) -> bool {
        self.token.is_some()
    }

    /// The last rate-limit headers the server sent.
    pub fn rate_limit(&self) -> RateLimit {
        *self
            .rate_limit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The URL for an API v4 path such as `users/me`.
    pub fn api_url(&self, path: &str) -> Result<Url, ApiError> {
        self.base
            .join("api/v4/")
            .and_then(|api| api.join(path.trim_start_matches('/')))
            .map_err(|e| ApiError::InvalidUrl(e.to_string()))
    }

    /// `GET` an API path and decode its JSON.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        self.json(Method::GET, path, &[], None::<&()>).await
    }

    /// `GET` with query parameters.
    pub async fn get_query<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ApiError> {
        self.json(Method::GET, path, query, None::<&()>).await
    }

    /// `POST` a JSON body and decode the JSON answer.
    pub async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        self.json(Method::POST, path, &[], Some(body)).await
    }

    /// `PUT` a JSON body and decode the JSON answer.
    pub async fn put<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        self.json(Method::PUT, path, &[], Some(body)).await
    }

    /// `DELETE` an API path and decode the JSON answer.
    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        self.json(Method::DELETE, path, &[], None::<&()>).await
    }

    async fn json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&B>,
    ) -> Result<T, ApiError> {
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|e| ApiError::Decode(e.to_string()))?;
        let response = self.send(method, path, query, body).await?;
        let bytes = response.bytes().await.map_err(ApiError::from_transport)?;
        serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(e.to_string()))
    }

    /// Sends a request and returns the successful response, so callers can
    /// read headers (the login token comes back in `Token`). Retries a 429
    /// up to [`MAX_TRIES`] times in all, waiting for the reset the server
    /// names, or 1s and then 2s when it names none.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Vec<u8>>,
    ) -> Result<reqwest::Response, ApiError> {
        self.send_with(method, path, query, body, &[]).await
    }

    /// `GET` raw bytes, such as an avatar or emoji image. With `etag`, sends
    /// `If-None-Match` and returns [`Fetched::NotModified`] on a 304.
    pub async fn get_bytes(&self, path: &str, etag: Option<&str>) -> Result<Fetched, ApiError> {
        self.get_bytes_query(path, &[], etag).await
    }

    /// [`Client::get_bytes`] with query parameters.
    pub async fn get_bytes_query(
        &self,
        path: &str,
        query: &[(&str, String)],
        etag: Option<&str>,
    ) -> Result<Fetched, ApiError> {
        let headers: Vec<(header::HeaderName, String)> = etag
            .map(|etag| (header::IF_NONE_MATCH, etag.to_string()))
            .into_iter()
            .collect();
        let response = self
            .send_with(Method::GET, path, query, None, &headers)
            .await?;
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(Fetched::NotModified);
        }
        let text = |name: header::HeaderName| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let etag = text(header::ETAG);
        let content_type = text(header::CONTENT_TYPE);
        let bytes = response.bytes().await.map_err(ApiError::from_transport)?;
        Ok(Fetched::Fresh {
            bytes: bytes.to_vec(),
            etag,
            content_type,
        })
    }

    /// `GET url` on an outside host, for an image the server does not
    /// proxy: no session token, only `http` and `https`, an `image/*`
    /// answer of at most [`MAX_OUTSIDE_IMAGE`] bytes.
    pub async fn outside_image(&self, url: &str) -> Result<Fetched, ApiError> {
        let parsed = Url::parse(url).map_err(|e| ApiError::InvalidUrl(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ApiError::InvalidUrl(format!("not a web link: {url}")));
        }
        let response = self
            .http
            .get(parsed)
            .send()
            .await
            .map_err(ApiError::from_transport)?;
        let response = Self::checked(response).await?;
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        if !content_type
            .as_deref()
            .is_some_and(|kind| kind.starts_with("image/"))
        {
            return Err(ApiError::Decode(format!(
                "not an image: {}",
                content_type.unwrap_or_default()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_OUTSIDE_IMAGE)
        {
            return Err(ApiError::Decode("image too large".into()));
        }
        let bytes = response.bytes().await.map_err(ApiError::from_transport)?;
        if bytes.len() as u64 > MAX_OUTSIDE_IMAGE {
            return Err(ApiError::Decode("image too large".into()));
        }
        Ok(Fetched::Fresh {
            bytes: bytes.to_vec(),
            etag: None,
            content_type,
        })
    }

    async fn send_with(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Vec<u8>>,
        extra_headers: &[(header::HeaderName, String)],
    ) -> Result<reqwest::Response, ApiError> {
        let url = self.api_url(path)?;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .header(header::ACCEPT, "application/json")
                .header("X-Requested-With", "XMLHttpRequest");
            if !query.is_empty() {
                request = request.query(query);
            }
            if let Some(token) = &self.token {
                request = request.bearer_auth(&token.0);
            }
            for (name, value) in extra_headers {
                request = request.header(name, value);
            }
            if let Some(body) = &body {
                request = request
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body.clone());
            }
            let response = request.send().await.map_err(ApiError::from_transport)?;
            let limit = self.note_rate_limit(&response);
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                if attempt >= MAX_TRIES {
                    return Err(ApiError::RateLimited);
                }
                self.sleeper
                    .sleep(backoff(attempt, limit, response.headers()))
                    .await;
                continue;
            }
            return Self::checked(response).await;
        }
    }

    fn note_rate_limit(&self, response: &reqwest::Response) -> Option<RateLimit> {
        let limit = RateLimit::from_headers(response.headers());
        if let Some(limit) = limit {
            *self.rate_limit.lock().unwrap_or_else(|p| p.into_inner()) = limit;
        }
        limit
    }

    /// The response if it succeeded (or is a 304), otherwise its error.
    async fn checked(response: reqwest::Response) -> Result<reqwest::Response, ApiError> {
        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_MODIFIED {
            return Ok(response);
        }
        if status.is_redirection() {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            return Err(ApiError::Redirected { location });
        }
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.bytes().await.unwrap_or_default();
        Err(ApiError::from_response(status, request_id, &bytes))
    }

    /// `GET path` once, allowing [`UPLOAD_TIMEOUT`] for the whole body, for
    /// a file read as it streams in.
    pub(crate) async fn send_long_get(&self, path: &str) -> Result<reqwest::Response, ApiError> {
        let mut request = self
            .http
            .get(self.api_url(path)?)
            .header("X-Requested-With", "XMLHttpRequest")
            .timeout(UPLOAD_TIMEOUT);
        if let Some(token) = &self.token {
            request = request.bearer_auth(&token.0);
        }
        let response = request.send().await.map_err(ApiError::from_transport)?;
        self.note_rate_limit(&response);
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(ApiError::RateLimited);
        }
        Self::checked(response).await
    }

    /// Sends a multipart form once: a streamed body cannot be replayed, so a
    /// 429 is [`ApiError::RateLimited`] at once. Allows [`UPLOAD_TIMEOUT`].
    pub(crate) async fn send_multipart(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<reqwest::Response, ApiError> {
        let mut request = self
            .http
            .post(self.api_url(path)?)
            .header(header::ACCEPT, "application/json")
            .header("X-Requested-With", "XMLHttpRequest")
            .timeout(UPLOAD_TIMEOUT)
            .multipart(form);
        if let Some(token) = &self.token {
            request = request.bearer_auth(&token.0);
        }
        let response = request.send().await.map_err(ApiError::from_transport)?;
        self.note_rate_limit(&response);
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(ApiError::RateLimited);
        }
        Self::checked(response).await
    }
}

fn backoff(attempt: u32, limit: Option<RateLimit>, headers: &header::HeaderMap) -> Duration {
    let named = limit.and_then(|l| l.reset).or_else(|| {
        headers
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
    });
    let wait = match named {
        Some(seconds) => Duration::from_secs(seconds.max(1)),
        None => Duration::from_secs(1 << (attempt - 1)),
    };
    wait.min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls_get_a_scheme_and_a_trailing_slash() {
        let client = Client::new("chat.example.com").unwrap();
        assert_eq!(client.base_url().as_str(), "https://chat.example.com/");
        let client = Client::new("http://localhost:8065/mattermost?x=1").unwrap();
        assert_eq!(
            client.base_url().as_str(),
            "http://localhost:8065/mattermost/"
        );
        assert_eq!(
            client.api_url("users/me").unwrap().as_str(),
            "http://localhost:8065/mattermost/api/v4/users/me"
        );
        assert!(matches!(
            Client::new("ftp://x"),
            Err(ApiError::InvalidUrl(_))
        ));
        assert!(matches!(Client::new(""), Err(ApiError::InvalidUrl(_))));
    }

    #[test]
    fn debug_never_shows_the_token() {
        let client = Client::new("https://chat.example.com")
            .unwrap()
            .with_token("sekrit-token-123");
        let printed = format!("{client:?} {client:#?}");
        assert!(!printed.contains("sekrit-token-123"), "{printed}");
        assert!(printed.contains("[redacted]"));
    }

    #[test]
    fn backoff_prefers_the_server_reset() {
        let mut headers = header::HeaderMap::new();
        let limit = RateLimit {
            limit: Some(10),
            remaining: Some(0),
            reset: Some(3),
        };
        assert_eq!(backoff(1, Some(limit), &headers), Duration::from_secs(3));
        headers.insert(header::RETRY_AFTER, "5".parse().unwrap());
        assert_eq!(backoff(1, None, &headers), Duration::from_secs(5));
        let none = header::HeaderMap::new();
        assert_eq!(backoff(1, None, &none), Duration::from_secs(1));
        assert_eq!(backoff(2, None, &none), Duration::from_secs(2));
        let huge = RateLimit {
            reset: Some(600),
            ..limit
        };
        assert_eq!(backoff(1, Some(huge), &none), MAX_BACKOFF);
    }
}
