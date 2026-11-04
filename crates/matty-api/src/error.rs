//! Errors from the Mattermost API, mapped so the UI can say something specific.

use reqwest::StatusCode;
use serde::Deserialize;

/// Everything a request can fail with.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ApiError {
    /// The server answered with an error body (`{"id", "message", ...}`).
    #[error("{message} ({id}, HTTP {status})")]
    Server {
        /// HTTP status.
        status: u16,
        /// Mattermost's error id, such as `api.user.login.invalid_credentials`.
        id: String,
        /// The server's human-readable message.
        message: String,
        /// `request_id` from the body or the `X-Request-Id` header, for support.
        request_id: Option<String>,
    },
    /// The account needs a multi-factor code, or the code was wrong.
    #[error("enter the code from your authenticator app")]
    MfaRequired,
    /// The server kept answering 429 after the retries ran out.
    #[error("the server is rate limiting requests; try again in a moment")]
    RateLimited,
    /// The TLS handshake failed, usually because of the server's certificate.
    #[error("{0}")]
    Tls(String),
    /// The request timed out.
    #[error("the server did not answer in time")]
    Timeout,
    /// The server could not be reached.
    #[error("could not reach the server: {0}")]
    Network(String),
    /// The response was not the JSON Matty expected.
    #[error("unexpected response from the server: {0}")]
    Decode(String),
    /// The server redirected to another host, which Matty never follows, so
    /// a session token or a request never leaves the server it was meant for.
    /// The image proxy answers this way when the server's proxy is off.
    #[error("the server redirected to another host: {location}")]
    Redirected {
        /// The `Location` the server named.
        location: String,
    },
    /// The URL Matty was given is not usable.
    #[error("invalid server URL: {0}")]
    InvalidUrl(String),
    /// Writing what the server sent failed on this computer, such as a
    /// download whose folder is full or read-only.
    #[error("{0}")]
    Io(String),
}

#[derive(Deserialize)]
struct ErrorBody {
    id: Option<String>,
    message: Option<String>,
    request_id: Option<String>,
}

impl ApiError {
    /// Builds an error from a non-success response body.
    pub(crate) fn from_response(
        status: StatusCode,
        header_request_id: Option<String>,
        body: &[u8],
    ) -> Self {
        match serde_json::from_slice::<ErrorBody>(body) {
            Ok(parsed) => ApiError::Server {
                status: status.as_u16(),
                id: parsed.id.unwrap_or_default(),
                message: parsed
                    .message
                    .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_string()),
                request_id: parsed
                    .request_id
                    .filter(|id| !id.is_empty())
                    .or(header_request_id),
            },
            Err(_) => ApiError::Server {
                status: status.as_u16(),
                id: String::new(),
                message: status.canonical_reason().unwrap_or("error").to_string(),
                request_id: header_request_id,
            },
        }
    }

    /// The Mattermost error id, when the server sent one.
    pub fn id(&self) -> Option<&str> {
        match self {
            ApiError::Server { id, .. } if !id.is_empty() => Some(id),
            _ => None,
        }
    }

    /// Maps a transport error, pulling out certificate problems.
    pub(crate) fn from_transport(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            return ApiError::Timeout;
        }
        if let Some(tls) = tls_cause(&error) {
            return ApiError::Tls(tls);
        }
        if error.is_decode() {
            return ApiError::Decode(error.to_string());
        }
        ApiError::Network(chain(&error))
    }
}

fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Finds a rustls error anywhere in the cause chain and words it for a person.
///
/// `std::io::Error::source` skips the error it wraps, and hyper nests one
/// `io::Error` inside another, so wrapped errors are followed with `get_ref`.
#[cfg(target_family = "wasm")]
pub(crate) fn tls_cause(_: &(dyn std::error::Error + 'static)) -> Option<String> {
    None
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn tls_cause(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(cause) = source {
        if let Some(rustls) = cause.downcast_ref::<rustls::Error>() {
            return Some(describe_tls(rustls));
        }
        source = match cause
            .downcast_ref::<std::io::Error>()
            .and_then(|io| io.get_ref())
        {
            Some(inner) => Some(inner as &(dyn std::error::Error + 'static)),
            None => cause.source(),
        };
    }
    None
}

#[cfg(not(target_family = "wasm"))]
fn describe_tls(error: &rustls::Error) -> String {
    use rustls::{CertificateError, Error};
    match error {
        Error::InvalidCertificate(CertificateError::UnknownIssuer) => {
            "The server's certificate is not issued by an authority this computer trusts. If your organization uses its own certificate authority, install it in the system's trust store.".into()
        }
        Error::InvalidCertificate(CertificateError::Expired | CertificateError::ExpiredContext { .. }) => {
            "The server's certificate has expired. Ask the server's administrator to renew it.".into()
        }
        Error::InvalidCertificate(
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
        ) => "The server's certificate is for a different address. Check the server URL.".into(),
        Error::InvalidCertificate(other) => {
            format!("The server's certificate could not be verified ({other:?}).")
        }
        other => format!("A secure connection to the server could not be set up ({other})."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_mattermost_error_body() {
        let body = br#"{"id":"api.user.login.invalid_credentials_email_username","message":"Enter a valid email or username and/or password.","detailed_error":"","request_id":"abc123","status_code":401}"#;
        let error = ApiError::from_response(StatusCode::UNAUTHORIZED, None, body);
        assert_eq!(
            error,
            ApiError::Server {
                status: 401,
                id: "api.user.login.invalid_credentials_email_username".into(),
                message: "Enter a valid email or username and/or password.".into(),
                request_id: Some("abc123".into()),
            }
        );
        assert_eq!(
            error.id(),
            Some("api.user.login.invalid_credentials_email_username")
        );
    }

    #[test]
    fn a_body_that_is_not_json_still_gives_a_status() {
        let error =
            ApiError::from_response(StatusCode::BAD_GATEWAY, Some("req-1".into()), b"<html>");
        assert_eq!(
            error,
            ApiError::Server {
                status: 502,
                id: String::new(),
                message: "Bad Gateway".into(),
                request_id: Some("req-1".into()),
            }
        );
        assert_eq!(error.id(), None);
    }
}
