use gotify_core::api::{ApiError, GotifyClient, create_client, endpoint};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn msg(id: i64) -> serde_json::Value {
    json!({"id": id, "appid": 1, "message": format!("m{id}"), "title": "t", "priority": 5, "date": "2026-11-04T15:10:07Z"})
}

fn page(ids: impl Iterator<Item = i64>, since: i64, next: Option<&str>) -> serde_json::Value {
    let messages: Vec<_> = ids.map(msg).collect();
    json!({"paging": {"size": messages.len(), "since": since, "limit": 200, "next": next}, "messages": messages})
}

#[test]
fn urls_respect_path_prefix() {
    for base in ["https://h/gotify/", "https://h/gotify"] {
        assert_eq!(endpoint(&base.parse().unwrap(), "message").as_str(), "https://h/gotify/message");
    }
    assert_eq!(endpoint(&"https://h".parse().unwrap(), "stream").as_str(), "https://h/stream");
}

#[tokio::test]
async fn sends_token_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/application"))
        .and(header("X-Gotify-Key", "T"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 1, "name": "Kuma", "description": "", "image": "static/defaultapp.png", "defaultPriority": 0}
        ])))
        .mount(&server)
        .await;
    let client = GotifyClient::new(server.uri().parse().unwrap(), "T".into());
    assert_eq!(client.applications().await.unwrap()[0].name, "Kuma");
}

#[tokio::test]
async fn all_messages_pages_with_since() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/message"))
        .and(query_param("since", "101"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page([100, 99].into_iter(), 99, None)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/message"))
        .and(query_param("limit", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(
            (101..=300).rev(),
            101,
            Some("/message?limit=200&since=101"),
        )))
        .with_priority(10)
        .mount(&server)
        .await;
    let client = GotifyClient::new(server.uri().parse().unwrap(), "T".into());
    let all = client.all_messages().await.unwrap();
    assert_eq!(all.len(), 202);
    assert_eq!(all.last().unwrap().id, 99);
}

#[tokio::test]
async fn newer_than_stops_early() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/message"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page((1..=10).rev(), 1, Some("/message?since=1"))))
        .expect(1)
        .mount(&server)
        .await;
    let client = GotifyClient::new(server.uri().parse().unwrap(), "T".into());
    let ids: Vec<_> = client.messages_newer_than(7).await.unwrap().iter().map(|m| m.id).collect();
    assert_eq!(ids, [10, 9, 8]);
}

#[tokio::test]
async fn maps_401_and_403() {
    let server = MockServer::start().await;
    Mock::given(path("/application"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "Unauthorized", "errorCode": 401, "errorDescription": "you need to provide a valid access token"})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/client/11"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"error": "Forbidden", "errorCode": 403, "errorDescription": "elevation required"})))
        .mount(&server)
        .await;
    Mock::given(path("/version"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"error": "x", "errorCode": 500, "errorDescription": "boom"})))
        .mount(&server)
        .await;
    let client = GotifyClient::new(server.uri().parse().unwrap(), "T".into());
    assert!(matches!(client.applications().await, Err(ApiError::Unauthorized)));
    assert!(matches!(client.delete_client(11).await, Err(ApiError::Forbidden)));
    assert!(matches!(client.version().await, Err(ApiError::Http(500, ref d)) if d == "boom"));
}

#[tokio::test]
async fn create_client_uses_basic_auth() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/client"))
        .and(header("Authorization", "Basic dTpw"))
        .and(body_json(json!({"name": "Ubuntu tray (host)"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 12, "token": "CTOKEN", "name": "Ubuntu tray (host)"})))
        .mount(&server)
        .await;
    let c = create_client(&server.uri().parse().unwrap(), "u", "p", "Ubuntu tray (host)").await.unwrap();
    assert_eq!((c.id, c.token.as_str()), (12, "CTOKEN"));
}

#[tokio::test]
async fn token_is_not_sent_to_another_host_on_redirect() {
    let gotify = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    Mock::given(path("/application"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", format!("{}/steal", elsewhere.uri()).as_str()))
        .mount(&gotify)
        .await;
    Mock::given(path("/steal")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).expect(0).mount(&elsewhere).await;
    let client = GotifyClient::new(gotify.uri().parse().unwrap(), "T".into());
    assert!(client.applications().await.is_err());
    elsewhere.verify().await;
}

#[tokio::test]
async fn same_host_redirect_is_followed() {
    let server = MockServer::start().await;
    Mock::given(path("/application"))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", format!("{}/gotify/application", server.uri()).as_str()))
        .mount(&server)
        .await;
    Mock::given(path("/gotify/application"))
        .and(header("X-Gotify-Key", "T"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let client = GotifyClient::new(server.uri().parse().unwrap(), "T".into());
    assert_eq!(client.applications().await.unwrap().len(), 0);
}
