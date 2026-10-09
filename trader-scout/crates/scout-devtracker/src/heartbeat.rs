//! Dead man's switch (`SCOUT_HEARTBEAT_URL`): the daemon sends a plain GET to an
//! external monitor's ping / heartbeat URL every minute while it and its
//! database are alive; the monitor alerts when the pings stop (a dead server
//! cannot report itself). Any service with such a URL works (healthchecks.io,
//! Better Stack heartbeats, Cronitor). On healthchecks.io (`hc-ping.com`) a
//! clean stop is also written to the check's log (`<url>/log`, no alert). The
//! URL is a secret: it is never printed.

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

    /// "Alive" (a plain GET, understood by every heartbeat service).
    ///
    /// # Errors
    /// Transport failure or a non-2xx answer (text without the URL).
    pub async fn ping(&self) -> Result<(), String> {
        Self::check(self.http.get(self.endpoint("")).send().await)
    }

    /// healthchecks.io only: a log entry (no alert), e.g. a planned stop.
    /// Other services have no such endpoint: `Ok(false)`, nothing sent.
    ///
    /// # Errors
    /// As [`Heartbeat::ping`].
    pub async fn log(&self, message: &str) -> Result<bool, String> {
        if !self.supports_log() {
            return Ok(false);
        }
        let resp = self
            .http
            .post(self.endpoint("/log"))
            .body(message.to_string())
            .send()
            .await;
        Self::check(resp).map(|()| true)
    }

    fn supports_log(&self) -> bool {
        self.url
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .is_some_and(|host| host == "hc-ping.com" || host.ends_with(".hc-ping.com"))
    }

    fn check(resp: Result<reqwest::Response, reqwest::Error>) -> Result<(), String> {
        let resp = resp.map_err(|e| e.without_url().to_string())?;
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
        assert!(h.supports_log());
        let other = Heartbeat {
            url: "https://uptime.betterstack.com/api/v1/heartbeat/secret".into(),
            http: reqwest::Client::new(),
        };
        assert!(!other.supports_log(), "only healthchecks has /log");
    }
}
