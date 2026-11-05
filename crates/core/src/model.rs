//! Gotify API models (only the fields we use; unknown fields are ignored).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub appid: i64,
    pub message: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub priority: i64,
    pub date: DateTime<Utc>,
    #[serde(default)]
    pub extras: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Application {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub image: String,
    #[serde(default, rename = "defaultPriority")]
    pub default_priority: i64,
}

impl Application {
    /// False for Gotify's generic placeholder icon.
    pub fn has_custom_image(&self) -> bool {
        !self.image.is_empty() && !self.image.contains("static/defaultapp.png")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paging {
    pub size: i64,
    pub since: i64,
    pub limit: i64,
    #[serde(default)]
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PagedMessages {
    pub paging: Paging,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GotifyInfo {
    pub version: String,
    #[serde(rename = "localAuth")]
    pub local_auth: bool,
    pub oidc: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub name: String,
}

/// Response of `POST /client`; the token is only ever returned here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewClient {
    pub id: i64,
    pub token: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_paged_messages() {
        let json = include_str!("../tests/fixtures/messages.json");
        let page: PagedMessages = serde_json::from_str(json).unwrap();
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.paging.next.as_deref(), Some("/message?limit=2&since=5"));
        assert_eq!(page.messages[1].title, None);
        assert_eq!(page.messages[0].date.timestamp_subsec_nanos(), 736_095_042);
        assert!(page.messages[0].extras.is_some());
    }

    #[test]
    fn default_image_is_not_custom() {
        let mut app: Application = serde_json::from_str(
            r#"{"id":3,"name":"Kuma","description":"","image":"static/defaultapp.png","defaultPriority":0,"internal":false}"#,
        )
        .unwrap();
        assert!(!app.has_custom_image());
        app.image = "image/abc.png".into();
        assert!(app.has_custom_image());
        app.image = String::new();
        assert!(!app.has_custom_image());
    }

    #[test]
    fn gotify_info_parses() {
        let info: GotifyInfo = serde_json::from_str(
            r#"{"version":"3.1.0","register":false,"localAuth":true,"oidc":false,"oidcIdpName":"OIDC","oidcAutoRedirect":false}"#,
        )
        .unwrap();
        assert!(info.local_auth);
        assert!(!info.oidc);
        assert_eq!(info.version, "3.1.0");
    }
}
