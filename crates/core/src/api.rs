//! Gotify HTTP API client.

use std::time::Duration;

use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use url::Url;

use crate::consts::PAGE_LIMIT;
use crate::model::{Application, GotifyInfo, Message, NewClient, PagedMessages, User, VersionInfo};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("the server rejected the login token")]
    Unauthorized,
    #[error("the server refused this action")]
    Forbidden,
    #[error("server error {0}: {1}")]
    Http(u16, String),
    #[error("can't reach the server: {0}")]
    Network(String),
    #[error("unexpected server response: {0}")]
    Decode(String),
}

/// `path` is joined under the base URL's path, so `https://h/gotify` works.
pub fn endpoint(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    let joined = format!("{}/{}", base.path().trim_end_matches('/'), path.trim_start_matches('/'));
    url.set_path(&joined);
    url.set_query(None);
    url
}

/// Follow redirects only within the same origin: the token travels in a
/// custom header, which reqwest would otherwise send to any redirect target.
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let same_origin = attempt.previous().first().is_some_and(|first| first.origin() == attempt.url().origin());
        if same_origin && attempt.previous().len() < 10 { attempt.follow() } else { attempt.stop() }
    })
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(same_origin_redirects())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("gotify-tray/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("HTTP client config is static")
}

async fn send(req: RequestBuilder) -> Result<Response, ApiError> {
    let resp = req.send().await.map_err(|e| ApiError::Network(e.to_string()))?;
    match resp.status() {
        s if s.is_success() => Ok(resp),
        StatusCode::UNAUTHORIZED => Err(ApiError::Unauthorized),
        StatusCode::FORBIDDEN => Err(ApiError::Forbidden),
        s => {
            let body = resp.text().await.unwrap_or_default();
            let description = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["errorDescription"].as_str().map(str::to_string))
                .unwrap_or(body);
            Err(ApiError::Http(s.as_u16(), description))
        }
    }
}

async fn json<T: DeserializeOwned>(req: RequestBuilder) -> Result<T, ApiError> {
    send(req).await?.json().await.map_err(|e| ApiError::Decode(e.to_string()))
}

#[derive(Debug, Clone)]
pub struct GotifyClient {
    base: Url,
    token: String,
    http: reqwest::Client,
}

impl GotifyClient {
    pub fn new(base: Url, token: String) -> Self {
        GotifyClient { base, token, http: http() }
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    fn req(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        self.http.request(method, endpoint(&self.base, path)).header("X-Gotify-Key", &self.token)
    }

    pub async fn version(&self) -> Result<VersionInfo, ApiError> {
        json(self.req(reqwest::Method::GET, "version")).await
    }

    pub async fn current_user(&self) -> Result<User, ApiError> {
        json(self.req(reqwest::Method::GET, "current/user")).await
    }

    pub async fn applications(&self) -> Result<Vec<Application>, ApiError> {
        json(self.req(reqwest::Method::GET, "application")).await
    }

    /// One page (newest first), optionally for one app, older than `since`.
    pub async fn messages_page(&self, app: Option<i64>, since: Option<i64>) -> Result<PagedMessages, ApiError> {
        let path = match app {
            Some(id) => format!("application/{id}/message"),
            None => "message".into(),
        };
        let mut req = self.req(reqwest::Method::GET, &path).query(&[("limit", PAGE_LIMIT as i64)]);
        if let Some(since) = since {
            req = req.query(&[("since", since)]);
        }
        json(req).await
    }

    pub async fn all_messages(&self) -> Result<Vec<Message>, ApiError> {
        self.all_messages_of(None).await
    }

    /// Full history, newest first, optionally for one app.
    pub async fn all_messages_of(&self, app: Option<i64>) -> Result<Vec<Message>, ApiError> {
        self.collect_pages(app, |_| false).await
    }

    /// Messages with an id above `id`, newest first; stops paging early.
    pub async fn messages_newer_than(&self, id: i64) -> Result<Vec<Message>, ApiError> {
        let mut msgs = self.collect_pages(None, |page| page.messages.iter().any(|m| m.id <= id)).await?;
        msgs.retain(|m| m.id > id);
        Ok(msgs)
    }

    async fn collect_pages(&self, app: Option<i64>, stop: impl Fn(&PagedMessages) -> bool) -> Result<Vec<Message>, ApiError> {
        let mut all = Vec::new();
        let mut since = None;
        loop {
            let page = self.messages_page(app, since).await?;
            let done = stop(&page) || page.paging.next.as_deref().is_none_or(str::is_empty) || page.messages.is_empty();
            since = Some(page.paging.since);
            all.extend(page.messages);
            if done {
                return Ok(all);
            }
        }
    }

    pub async fn delete_message(&self, id: i64) -> Result<(), ApiError> {
        send(self.req(reqwest::Method::DELETE, &format!("message/{id}"))).await.map(drop)
    }

    pub async fn delete_app_messages(&self, app_id: i64) -> Result<(), ApiError> {
        send(self.req(reqwest::Method::DELETE, &format!("application/{app_id}/message"))).await.map(drop)
    }

    pub async fn delete_all_messages(&self) -> Result<(), ApiError> {
        send(self.req(reqwest::Method::DELETE, "message")).await.map(drop)
    }

    /// Needs an elevated session in Gotify 3, so usually `Forbidden`.
    pub async fn delete_client(&self, id: i64) -> Result<(), ApiError> {
        send(self.req(reqwest::Method::DELETE, &format!("client/{id}"))).await.map(drop)
    }

    /// App icon bytes; `path` is `Application::image` (relative).
    pub async fn image(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        let resp = send(self.req(reqwest::Method::GET, path)).await?;
        resp.bytes().await.map(|b| b.to_vec()).map_err(|e| ApiError::Network(e.to_string()))
    }
}

/// Unauthenticated: version and which login methods the server allows.
pub async fn server_info(base: &Url) -> Result<GotifyInfo, ApiError> {
    json(http().get(endpoint(base, "gotifyinfo"))).await
}

/// Log in like the Android app: create a client token with the password.
pub async fn create_client(base: &Url, user: &str, password: &str, name: &str) -> Result<NewClient, ApiError> {
    let req = http()
        .post(endpoint(base, "client"))
        .basic_auth(user, Some(password))
        .json(&serde_json::json!({ "name": name }));
    json(req).await
}

/// Removes a client using the password (Basic auth is always elevated in
/// Gotify 3), e.g. to clean up after a login that failed halfway.
pub async fn delete_client_with_password(base: &Url, user: &str, password: &str, id: i64) -> Result<(), ApiError> {
    send(http().delete(endpoint(base, &format!("client/{id}"))).basic_auth(user, Some(password))).await.map(drop)
}
