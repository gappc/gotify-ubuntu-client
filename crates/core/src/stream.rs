//! Gotify WebSocket stream (`/stream`): new messages as they arrive.
//!
//! The server pings regularly; any frame resets a silence timer, so a
//! half-dead connection (e.g. after suspend) is noticed and reported.

use std::time::Duration;

use futures_util::StreamExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message as Ws};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::api::{ApiError, endpoint};
use crate::model::Message;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEnd {
    Closed,
    Silent,
    Unauthorized,
    Error(String),
}

pub struct MessageStream {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    silence: Duration,
}

fn stream_url(base: &Url) -> Result<Url, ApiError> {
    let mut url = endpoint(base, "stream");
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        other => return Err(ApiError::Network(format!("unsupported URL scheme {other}"))),
    };
    url.set_scheme(scheme).map_err(|()| ApiError::Network("bad stream URL".into()))?;
    Ok(url)
}

pub async fn connect_stream(base: &Url, token: &str, silence: Duration) -> Result<MessageStream, ApiError> {
    let mut req = stream_url(base)?
        .as_str()
        .into_client_request()
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let key = token.parse().map_err(|_| ApiError::Unauthorized)?;
    req.headers_mut().insert("X-Gotify-Key", key);
    let connect = tokio_tungstenite::connect_async(req);
    match tokio::time::timeout(Duration::from_secs(15), connect).await {
        Err(_) => Err(ApiError::Network("timed out connecting to the stream".into())),
        Ok(Ok((ws, _))) => Ok(MessageStream { ws, silence }),
        Ok(Err(tungstenite::Error::Http(resp))) if resp.status().as_u16() == 401 => Err(ApiError::Unauthorized),
        Ok(Err(tungstenite::Error::Http(resp))) => {
            Err(ApiError::Http(resp.status().as_u16(), "stream handshake rejected".into()))
        }
        Ok(Err(e)) => Err(ApiError::Network(e.to_string())),
    }
}

impl MessageStream {
    /// The next message; `Err` means the stream is over and must be reopened.
    pub async fn next(&mut self) -> Result<Message, StreamEnd> {
        loop {
            let frame = match tokio::time::timeout(self.silence, self.ws.next()).await {
                Err(_) => return Err(StreamEnd::Silent),
                Ok(None) => return Err(StreamEnd::Closed),
                Ok(Some(Err(tungstenite::Error::ConnectionClosed))) => return Err(StreamEnd::Closed),
                Ok(Some(Err(e))) => return Err(StreamEnd::Error(e.to_string())),
                Ok(Some(Ok(frame))) => frame,
            };
            match frame {
                Ws::Text(text) => match serde_json::from_str(&text) {
                    Ok(msg) => return Ok(msg),
                    Err(e) => eprintln!("gotify-tray: skipping unreadable stream frame: {e}"),
                },
                Ws::Close(_) => return Err(StreamEnd::Closed),
                // Pings are answered by tungstenite; any frame counts as alive.
                _ => {}
            }
        }
    }
}
