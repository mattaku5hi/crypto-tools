//! Dead man's switch (healthchecks.io-style ping URL, `SCOUT_HEARTBEAT_URL`):
//! the daemon pings an external monitor every minute while it and its database
//! are alive; the monitor alerts when the pings stop (a dead server cannot
//! report itself). A clean stop is written to the monitor's log (`<url>/log`),
//! which does not alert. The URL is a secret: it is never printed.

use std::time::Duration;

/// Environment variable with the ping URL (empty or unset = off).
pub const HEARTBEAT_ENV: &str = "SCOUT_HEARTBEAT_URL";

/// A configured ping URL.
#[derive(Clone)]
pub struct Heartbeat {
    url: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for Heartbeat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Heartbeat").finish_non_exhaustive()
    }
}

impl Heartbeat {
    /// From [`HEARTBEAT_ENV`]; `None` when unset, empty or not an http(s) URL.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let url = std::env::var(HEARTBEAT_ENV).ok()?.trim().to_string();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return None;
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .ok()?;
        Some(Self { url, http })
    }

    fn endpoint(&self, suffix: &str) -> String {
        format!("{}{suffix}", self.url.trim_end_matches('/'))
    }

    /// "Alive". Failures are returned as text without the URL.
    ///
    /// # Errors
    /// Transport failure or a non-2xx answer.
    pub async fn ping(&self) -> Result<(), String> {
        self.send("", None).await
    }

    /// A log entry on the monitor (no alert), e.g. a planned stop.
    ///
    /// # Errors
    /// As [`Heartbeat::ping`].
    pub async fn log(&self, message: &str) -> Result<(), String> {
        self.send("/log", Some(message.to_string())).await
    }

    async fn send(&self, suffix: &str, body: Option<String>) -> Result<(), String> {
        let req = self.http.post(self.endpoint(suffix));
        let req = match body {
            Some(b) => req.body(b),
            None => req,
        };
        let resp = req.send().await.map_err(|e| e.without_url().to_string())?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("HTTP {}", resp.status()))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn endpoints_and_debug_never_show_the_url() {
        let h = Heartbeat {
            url: "https://hc-ping.com/secret-uuid/".into(),
            http: reqwest::Client::new(),
        };
        assert_eq!(h.endpoint(""), "https://hc-ping.com/secret-uuid");
        assert_eq!(h.endpoint("/log"), "https://hc-ping.com/secret-uuid/log");
        assert!(!format!("{h:?}").contains("secret"));
    }
}
