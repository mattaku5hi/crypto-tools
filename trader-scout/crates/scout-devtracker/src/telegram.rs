//! Telegram delivery of changed export files (B7): `sendDocument` to one chat.
//! The bot token is part of the request URL, so errors are reported without
//! the URL and the token never reaches a log.

use std::time::Duration;

/// Bot token and target chat (from `SCOUT_TELEGRAM_BOT_TOKEN` /
/// `SCOUT_TELEGRAM_CHAT_ID`).
#[derive(Clone)]
pub struct Telegram {
    token: String,
    chat_id: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for Telegram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Telegram")
            .field("chat_id", &self.chat_id)
            .finish_non_exhaustive()
    }
}

/// A failed send.
#[derive(Debug, thiserror::Error)]
#[error("telegram: {0}")]
pub struct TelegramError(String);

impl Telegram {
    /// # Errors
    /// HTTP client construction failure.
    pub fn new(token: &str, chat_id: &str) -> Result<Self, TelegramError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| TelegramError(e.without_url().to_string()))?;
        Ok(Self {
            token: token.trim().to_string(),
            chat_id: chat_id.trim().to_string(),
            http,
        })
    }

    /// Send one file with a caption.
    ///
    /// # Errors
    /// Transport failure or a non-`ok` Bot API answer (its `description` only).
    pub async fn send_document(
        &self,
        file_name: &str,
        content: Vec<u8>,
        caption: &str,
    ) -> Result<(), TelegramError> {
        let part = reqwest::multipart::Part::bytes(content).file_name(file_name.to_string());
        let form = reqwest::multipart::Form::new()
            .text("chat_id", self.chat_id.clone())
            .text("caption", caption.to_string())
            .part("document", part);
        let resp = self
            .http
            .post(format!(
                "https://api.telegram.org/bot{}/sendDocument",
                self.token
            ))
            .multipart(form)
            .send()
            .await
            .map_err(|e| TelegramError(e.without_url().to_string()))?;
        let status = resp.status();
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| TelegramError(e.without_url().to_string()))?;
        if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
            Ok(())
        } else {
            let desc = v
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no description");
            Err(TelegramError(format!(
                "HTTP {status}: {}",
                desc.replace(&self.token, "<redacted>")
                    .chars()
                    .take(200)
                    .collect::<String>()
            )))
        }
    }
}
