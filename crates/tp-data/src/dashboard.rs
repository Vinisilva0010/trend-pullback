//! Publishes the dashboard state to the web app (`POST {url}/api/ingest`).
//! The token goes only in the `Authorization` header and never in errors.

use std::time::Duration;

/// Errors while publishing to the dashboard.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("DASHBOARD_URL must start with https:// (or http://localhost for tests)")]
    Url,
    #[error("DASHBOARD_TOKEN must be 32 to 256 letters, digits, '-' or '_'")]
    Token,
    #[error("the dashboard answered with HTTP status {0}")]
    Status(u16),
    #[error("network error while publishing to the dashboard: {0}")]
    Network(reqwest::Error),
}

/// True for an https URL, or plain http only on localhost (local tests).
#[must_use]
pub fn is_valid_url(url: &str) -> bool {
    if url.contains(char::is_whitespace) {
        return false;
    }
    if let Some(r) = url.strip_prefix("http://localhost") {
        return r.is_empty() || r.starts_with(':') || r.starts_with('/');
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    !rest.is_empty() && !rest.contains('@')
}

/// True for a long random token of URL-safe characters.
#[must_use]
pub fn is_valid_token(token: &str) -> bool {
    (32..=256).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Sends `body` (JSON) to `{url}/api/ingest`.
///
/// # Errors
///
/// Returns [`PublishError`] for a bad URL or token, a network failure, or a
/// non-success answer.
pub async fn publish(url: &str, token: &str, body: String) -> Result<(), PublishError> {
    if !is_valid_url(url) {
        return Err(PublishError::Url);
    }
    if !is_valid_token(token) {
        return Err(PublishError::Token);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| PublishError::Network(e.without_url()))?;
    let response = client
        .post(format!("{}/api/ingest", url.trim_end_matches('/')))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .body(body)
        .send()
        .await
        .map_err(|e| PublishError::Network(e.without_url()))?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(PublishError::Status(status.as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_tokens_are_checked() {
        assert!(is_valid_url("https://painel.vercel.app"));
        assert!(is_valid_url("http://localhost:3000"));
        assert!(!is_valid_url("http://painel.vercel.app"));
        assert!(!is_valid_url("http://localhost.evil.com"));
        assert!(!is_valid_url("https://user@evil.com"));
        assert!(!is_valid_url("https://"));
        assert!(is_valid_token(&"a".repeat(43)));
        assert!(!is_valid_token("short"));
        assert!(!is_valid_token(&format!("{}\n", "a".repeat(40))));
    }
}
