use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use gotify_core::api::ApiError;
use gotify_core::stream::{StreamEnd, connect_stream};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

const MSG: &str = r#"{"id":7,"appid":1,"message":"[web01] [🔴 Down] x","title":"Uptime-Kuma","priority":8,"date":"2026-11-04T15:10:07Z"}"#;

/// Starts a one-connection WebSocket server running `script`; returns its base URL.
async fn server<F, Fut>(script: F) -> url::Url
where
    F: FnOnce(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
            assert_eq!(req.uri().path(), "/gotify/stream");
            assert_eq!(req.headers()["X-Gotify-Key"], "T");
            Ok(resp)
        };
        let ws = tokio_tungstenite::accept_hdr_async(tcp, check).await.unwrap();
        script(ws).await;
    });
    format!("http://{addr}/gotify/").parse().unwrap()
}

#[tokio::test]
async fn stream_yields_messages() {
    let base = server(|mut ws| async move {
        ws.send(Ws::text(MSG)).await.unwrap();
        ws.send(Ws::text(MSG.replace("\"id\":7", "\"id\":8"))).await.unwrap();
        ws.close(None).await.unwrap();
    })
    .await;
    let mut s = connect_stream(&base, "T", Duration::from_secs(5)).await.unwrap();
    assert_eq!(s.next().await.unwrap().id, 7);
    assert_eq!(s.next().await.unwrap().id, 8);
    assert_eq!(s.next().await.unwrap_err(), StreamEnd::Closed);
}

#[tokio::test]
async fn stream_reports_silence() {
    let base = server(|ws| async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        drop(ws);
    })
    .await;
    let mut s = connect_stream(&base, "T", Duration::from_millis(200)).await.unwrap();
    let start = Instant::now();
    assert_eq!(s.next().await.unwrap_err(), StreamEnd::Silent);
    assert!(start.elapsed() < Duration::from_millis(400));
}

#[tokio::test]
async fn pings_keep_stream_alive() {
    let base = server(|mut ws| async move {
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            ws.send(Ws::Ping(Vec::new().into())).await.unwrap();
        }
        ws.close(None).await.unwrap();
        while ws.next().await.is_some() {}
    })
    .await;
    let mut s = connect_stream(&base, "T", Duration::from_millis(200)).await.unwrap();
    assert_eq!(s.next().await.unwrap_err(), StreamEnd::Closed);
}

#[tokio::test]
async fn handshake_401_is_unauthorized() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let reject = |_: &Request, _: Response| -> Result<Response, ErrorResponse> {
            let mut r = ErrorResponse::new(Some("unauthorized".into()));
            *r.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
            Err(r)
        };
        let _ = tokio_tungstenite::accept_hdr_async(tcp, reject).await;
    });
    let base: url::Url = format!("http://{addr}/").parse().unwrap();
    let err = connect_stream(&base, "T", Duration::from_secs(1)).await.err().unwrap();
    assert_eq!(err, ApiError::Unauthorized);
}

#[tokio::test]
async fn skips_invalid_json() {
    let base = server(|mut ws| async move {
        ws.send(Ws::text("{not json")).await.unwrap();
        ws.send(Ws::text(MSG)).await.unwrap();
        ws.close(None).await.unwrap();
    })
    .await;
    let mut s = connect_stream(&base, "T", Duration::from_secs(5)).await.unwrap();
    assert_eq!(s.next().await.unwrap().id, 7);
}
