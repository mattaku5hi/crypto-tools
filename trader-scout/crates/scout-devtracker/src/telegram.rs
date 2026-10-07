//! Telegram delivery of changed export files (B7): one album of documents
//! (`sendMediaGroup`, or `sendDocument` for a single file) per changed list,
//! the change report as its caption, overflow as follow-up messages. The bot
//! token is part of the request URL, so errors are reported without the URL
//! and the token never reaches a log.

use std::time::Duration;

use serde_json::{Value, json};

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

/// Documents per album (Bot API limit).
pub const MAX_ALBUM: usize = 10;

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

    fn url(&self, method: &str) -> String {
        format!("https://api.telegram.org/bot{}/{method}", self.token)
    }

    async fn check(&self, resp: reqwest::Response) -> Result<(), TelegramError> {
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| TelegramError(e.without_url().to_string()))?;
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
        let desc = v
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("no description");
        Err(TelegramError(format!(
            "HTTP {status}: {}",
            desc.replace(&self.token, "<redacted>")
                .chars()
                .take(200)
                .collect::<String>()
        )))
    }

    /// Send one file with a caption (≤ 1,024 chars).
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
            .post(self.url("sendDocument"))
            .multipart(form)
            .send()
            .await
            .map_err(|e| TelegramError(e.without_url().to_string()))?;
        self.check(resp).await
    }

    /// Send 1..=10 files as one album; `caption` goes under the last one.
    ///
    /// # Errors
    /// As [`Telegram::send_document`]; more than [`MAX_ALBUM`] files.
    pub async fn send_album(
        &self,
        files: Vec<(String, Vec<u8>)>,
        caption: &str,
    ) -> Result<(), TelegramError> {
        if files.len() > MAX_ALBUM {
            return Err(TelegramError(format!(
                "{} files exceed an album of {MAX_ALBUM}",
                files.len()
            )));
        }
        if files.len() <= 1 {
            return match files.into_iter().next() {
                Some((name, content)) => self.send_document(&name, content, caption).await,
                None => Ok(()),
            };
        }
        let last = files.len() - 1;
        let mut media = Vec::with_capacity(files.len());
        let mut form = reqwest::multipart::Form::new().text("chat_id", self.chat_id.clone());
        for (i, (name, content)) in files.into_iter().enumerate() {
            let key = format!("file{i}");
            let item = if i == last {
                json!({"type": "document", "media": format!("attach://{key}"), "caption": caption})
            } else {
                json!({"type": "document", "media": format!("attach://{key}")})
            };
            media.push(item);
            form = form.part(
                key,
                reqwest::multipart::Part::bytes(content).file_name(name),
            );
        }
        form = form.text("media", Value::Array(media).to_string());
        let resp = self
            .http
            .post(self.url("sendMediaGroup"))
            .multipart(form)
            .send()
            .await
            .map_err(|e| TelegramError(e.without_url().to_string()))?;
        self.check(resp).await
    }

    /// Send a plain text message (≤ 4,096 chars).
    ///
    /// # Errors
    /// As [`Telegram::send_document`].
    pub async fn send_message(&self, text: &str) -> Result<(), TelegramError> {
        let resp = self
            .http
            .post(self.url("sendMessage"))
            .json(&json!({"chat_id": self.chat_id, "text": text, "disable_web_page_preview": true}))
            .send()
            .await
            .map_err(|e| TelegramError(e.without_url().to_string()))?;
        self.check(resp).await
    }
}
