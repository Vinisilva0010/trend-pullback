//! Telegram notifications through the Bot API. The token never appears in
//! errors or output: network errors are stripped of the request URL.

use std::time::Duration;

/// Errors while sending a notification.
#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("the Telegram bot token has an unexpected format")]
    Token,
    #[error("the Telegram chat id has an unexpected format")]
    ChatId,
    #[error("Telegram answered with HTTP status {0}")]
    Status(u16),
    #[error("network error while sending the Telegram message: {0}")]
    Network(reqwest::Error),
}

/// True for tokens shaped like `123456:ABC-def_...`.
#[must_use]
pub fn is_valid_token(token: &str) -> bool {
    let Some((id, secret)) = token.split_once(':') else {
        return false;
    };
    !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
        && secret.len() >= 20
        && secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// True for numeric chat ids, optionally negative (groups).
#[must_use]
pub fn is_valid_chat_id(chat_id: &str) -> bool {
    let digits = chat_id.strip_prefix('-').unwrap_or(chat_id);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Sends `text` to `chat_id`.
///
/// # Errors
///
/// Returns [`NotifyError`] for a malformed token or chat id, a network
/// failure, or a non-success answer from Telegram.
pub async fn send_telegram(token: &str, chat_id: &str, text: &str) -> Result<(), NotifyError> {
    if !is_valid_token(token) {
        return Err(NotifyError::Token);
    }
    if !is_valid_chat_id(chat_id) {
        return Err(NotifyError::ChatId);
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| NotifyError::Network(e.without_url()))?;
    let body = serde_json::json!({ "chat_id": chat_id, "text": text }).to_string();
    let response = client
        .post(format!("https://api.telegram.org/bot{token}/sendMessage"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| NotifyError::Network(e.without_url()))?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(NotifyError::Status(status.as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_token_and_chat_id() {
        assert!(is_valid_token("123456789:AAEhBP0av18bvsjA_Yd-ABCDEFGH"));
        assert!(!is_valid_token("123456789"));
        assert!(!is_valid_token("abc:AAEhBP0av18bvsjA_Yd-ABCDEFGH"));
        assert!(!is_valid_token("123:short"));
        assert!(!is_valid_token("123:AAEhBP0av18bvsjA/Yd?ABCDEFGH"));
        assert!(is_valid_chat_id("987654321"));
        assert!(is_valid_chat_id("-100123"));
        assert!(!is_valid_chat_id("12a"));
        assert!(!is_valid_chat_id(""));
    }
}
