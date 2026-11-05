//! Runs against the local Docker Gotify (`tests/seed.sh up`).
//! Skipped unless GOTIFY_TEST_URL is set.

use std::time::Duration;

use gotify_core::api::{ApiError, GotifyClient, create_client};
use gotify_core::kuma::{KumaStatus, parse_kuma};
use gotify_core::stream::connect_stream;

fn test_url() -> Option<url::Url> {
    match std::env::var("GOTIFY_TEST_URL") {
        Ok(u) => Some(u.parse().unwrap()),
        Err(_) => {
            eprintln!("GOTIFY_TEST_URL not set; skipping live test");
            None
        }
    }
}

async fn send_as_kuma(base: &url::Url, text: &str) {
    let token = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/.app-token")).unwrap();
    reqwest::Client::new()
        .post(gotify_core::api::endpoint(base, "message"))
        .header("X-Gotify-Key", token.trim())
        .json(&serde_json::json!({"title": "Uptime-Kuma", "message": text, "priority": 8}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
}

#[tokio::test]
async fn live_roundtrip() {
    let Some(base) = test_url() else { return };
    let new = create_client(&base, "admin", "admin", "Ubuntu tray (live-test)").await.unwrap();
    let client = GotifyClient::new(base.clone(), new.token.clone());
    let mut stream = connect_stream(&base, &new.token, Duration::from_secs(100)).await.unwrap();

    send_as_kuma(&base, "[web01] [🔴 Down] live test").await;
    let msg = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().unwrap();
    assert_eq!(parse_kuma(&msg.message).unwrap().status, KumaStatus::Down);

    assert!(client.all_messages().await.unwrap().iter().any(|m| m.id == msg.id));
    client.delete_message(msg.id).await.unwrap();
    assert!(!client.all_messages().await.unwrap().iter().any(|m| m.id == msg.id));

    // Gotify 3: deleting a client needs an elevated session.
    assert_eq!(client.delete_client(new.id).await, Err(ApiError::Forbidden));
}
