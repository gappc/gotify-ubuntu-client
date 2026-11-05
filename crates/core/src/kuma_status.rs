//! Uptime Kuma status page: the current state of every monitor on the page
//! (public API, no login; Kuma caches it for a minute).

use std::collections::HashMap;

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Deserialize;
use url::Url;

use crate::api::endpoint;

/// A status page address like `http://kuma:3001/status/<slug>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusPage {
    base: Url,
    slug: String,
}

impl StatusPage {
    pub fn parse(input: &str) -> Result<StatusPage, String> {
        let url = Url::parse(input.trim()).map_err(|_| "That doesn't look like a web address.".to_string())?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("The address must start with http:// or https://.".into());
        }
        let path = url.path().trim_end_matches('/');
        let (prefix, slug) = path
            .rsplit_once("/status/")
            .filter(|(_, slug)| !slug.is_empty() && !slug.contains('/'))
            .ok_or_else(|| "Use the status page's address, ending in /status/<slug>.".to_string())?;
        let mut base = url.clone();
        base.set_path(&format!("{prefix}/"));
        base.set_query(None);
        base.set_fragment(None);
        Ok(StatusPage { base, slug: slug.to_string() })
    }

    pub fn page_url(&self) -> Url {
        endpoint(&self.base, &format!("api/status-page/{}", self.slug))
    }

    pub fn heartbeat_url(&self) -> Url {
        endpoint(&self.base, &format!("api/status-page/heartbeat/{}", self.slug))
    }

    /// The address the user entered, for display and config.
    pub fn display_url(&self) -> Url {
        endpoint(&self.base, &format!("status/{}", self.slug))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KumaState {
    Up,
    Down,
    Pending,
    Maintenance,
    /// No checks yet.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KumaMonitor {
    pub name: String,
    pub state: KumaState,
    /// When the current state began (as far as the last 100 checks show).
    pub since: Option<DateTime<Utc>>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KumaSnapshot {
    pub monitors: Vec<KumaMonitor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub total: usize,
    pub up: usize,
    pub down: usize,
    pub maintenance: usize,
    pub pending: usize,
}

impl KumaSnapshot {
    pub fn counts(&self) -> Counts {
        let mut c = Counts { total: self.monitors.len(), ..Counts::default() };
        for m in &self.monitors {
            match m.state {
                KumaState::Up => c.up += 1,
                KumaState::Down => c.down += 1,
                KumaState::Maintenance => c.maintenance += 1,
                KumaState::Pending => c.pending += 1,
                KumaState::Unknown => {}
            }
        }
        c
    }
}

#[derive(Deserialize)]
struct Page {
    #[serde(rename = "publicGroupList")]
    groups: Vec<Group>,
}

#[derive(Deserialize)]
struct Group {
    #[serde(rename = "monitorList", default)]
    monitors: Vec<PageMonitor>,
}

#[derive(Deserialize)]
struct PageMonitor {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
struct Heartbeats {
    #[serde(rename = "heartbeatList")]
    list: HashMap<String, Vec<Beat>>,
}

#[derive(Deserialize)]
struct Beat {
    status: i64,
    time: String,
    #[serde(default)]
    msg: Option<String>,
}

fn state_of(status: i64) -> KumaState {
    match status {
        1 => KumaState::Up,
        0 => KumaState::Down,
        2 => KumaState::Pending,
        3 => KumaState::Maintenance,
        _ => KumaState::Unknown,
    }
}

/// Kuma sends UTC times without a zone: "2026-11-05 22:01:25.280".
fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").ok().map(|t| t.and_utc())
}

pub fn parse_snapshot(page_json: &str, heartbeat_json: &str) -> Result<KumaSnapshot, String> {
    let page: Page = serde_json::from_str(page_json).map_err(|_| "Not an Uptime Kuma status page.".to_string())?;
    let beats: Heartbeats =
        serde_json::from_str(heartbeat_json).map_err(|_| "Unexpected answer from Uptime Kuma.".to_string())?;
    let monitors = page
        .groups
        .iter()
        .flat_map(|g| &g.monitors)
        .map(|m| {
            let list = beats.list.get(&m.id.to_string()).map(Vec::as_slice).unwrap_or_default();
            let Some(last) = list.last() else {
                return KumaMonitor { name: m.name.clone(), state: KumaState::Unknown, since: None, message: String::new() };
            };
            // Walk back over the checks with the same status as the latest.
            let streak_start = list.iter().rev().take_while(|b| b.status == last.status).last().unwrap_or(last);
            KumaMonitor {
                name: m.name.clone(),
                state: state_of(last.status),
                since: parse_time(&streak_start.time),
                message: last.msg.clone().unwrap_or_default(),
            }
        })
        .collect();
    Ok(KumaSnapshot { monitors })
}

/// Fetches the page (monitor names) and the latest checks.
pub async fn fetch_snapshot(page: &StatusPage) -> Result<KumaSnapshot, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("gotify-tray/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let get = |url: Url| {
        let client = client.clone();
        async move {
            let resp = client.get(url).send().await.map_err(|e| format!("Can't reach Uptime Kuma: {e}"))?;
            if !resp.status().is_success() {
                return Err(format!("Uptime Kuma answered {}", resp.status()));
            }
            resp.text().await.map_err(|e| e.to_string())
        }
    };
    let (page_json, heartbeat_json) = tokio::try_join!(get(page.page_url()), get(page.heartbeat_url()))?;
    parse_snapshot(&page_json, &heartbeat_json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn snapshot() -> KumaSnapshot {
        parse_snapshot(
            include_str!("../tests/fixtures/kuma_page.json"),
            include_str!("../tests/fixtures/kuma_heartbeat.json"),
        )
        .unwrap()
    }

    #[test]
    fn page_url_splits_into_server_and_slug() {
        let p = StatusPage::parse("http://kuma.example.ts.net:3001/status/tray-abc").unwrap();
        assert_eq!(p.heartbeat_url().as_str(), "http://kuma.example.ts.net:3001/api/status-page/heartbeat/tray-abc");
        assert_eq!(p.page_url().as_str(), "http://kuma.example.ts.net:3001/api/status-page/tray-abc");
        let prefixed = StatusPage::parse(" https://h/kuma/status/s1/ ").unwrap();
        assert_eq!(prefixed.page_url().as_str(), "https://h/kuma/api/status-page/s1");
        assert!(StatusPage::parse("https://h/dashboard").is_err());
        assert!(StatusPage::parse("not a url").is_err());
    }

    #[test]
    fn monitors_keep_page_order_with_their_state() {
        let s = snapshot();
        let names: Vec<_> = s.monitors.iter().map(|m| (m.name.as_str(), m.state)).collect();
        assert_eq!(
            names,
            [
                ("web01", KumaState::Up),
                ("db", KumaState::Down),
                ("backup", KumaState::Maintenance),
                ("api", KumaState::Pending),
                ("new-monitor", KumaState::Unknown),
            ]
        );
    }

    #[test]
    fn down_since_is_the_start_of_the_current_outage() {
        let db = &snapshot().monitors[1];
        assert_eq!(db.since, Some(Utc.with_ymd_and_hms(2026, 11, 5, 21, 30, 0).unwrap() + chrono::Duration::milliseconds(123)));
        assert_eq!(db.message, "connect ECONNREFUSED");
    }

    #[test]
    fn counts_for_the_status_line() {
        let c = snapshot().counts();
        assert_eq!((c.total, c.up, c.down, c.maintenance, c.pending), (5, 1, 1, 1, 1));
    }

    #[test]
    fn broken_answers_are_errors() {
        assert!(parse_snapshot("Status Page Not Found", "{}").is_err());
        assert!(parse_snapshot(include_str!("../tests/fixtures/kuma_page.json"), "{}").is_err());
    }
}
