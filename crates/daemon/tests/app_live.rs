//! Drives the App against the local Docker Gotify (`tests/seed.sh up`).
//! Skipped unless GOTIFY_TEST_URL is set. Tests run one at a time.

use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gotify_core::api::{GotifyClient, create_client, endpoint};
use gotify_core::files::{Config, Paths, State, save_config, save_state};
use gotify_tray::app::{App, AppDeps, Event, Launcher, ServiceCall};
use gotify_tray::notify::{NotifyBackend, NotifyRequest, Notifier};
use gotify_tray::tray::TrayView;
use tokio::sync::watch;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<NotifyRequest>>>);
impl NotifyBackend for Recorder {
    async fn notify(&mut self, r: &NotifyRequest) -> u32 {
        let mut sent = self.0.lock().unwrap();
        sent.push(r.clone());
        sent.len() as u32
    }
    async fn close(&mut self, _id: u32) {}
}
impl Recorder {
    fn summaries(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().map(|r| r.summary.clone()).collect()
    }
}

struct NoLauncher;
impl Launcher for NoLauncher {
    fn window(&mut self, _args: Vec<String>) -> Option<tokio::process::Child> {
        None
    }
    fn open_link(&mut self, _url: String) {}
}

struct Running {
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    view: watch::Receiver<TrayView>,
    sent: Recorder,
    admin: GotifyClient,
    client_id: i64,
    _dir: tempfile::TempDir,
}

fn base() -> Option<url::Url> {
    std::env::var("GOTIFY_TEST_URL").ok().map(|u| u.parse().unwrap())
}

async fn send_as_kuma(base: &url::Url, text: &str, priority: i64) -> i64 {
    let token = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/.app-token")).unwrap();
    let v: serde_json::Value = reqwest::Client::new()
        .post(endpoint(base, "message"))
        .header("X-Gotify-Key", token.trim())
        .json(&serde_json::json!({"title": "Uptime-Kuma", "message": text, "priority": priority}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["id"].as_i64().unwrap()
}

/// A client token for "admin" plus the current newest message id.
async fn setup(base: &url::Url) -> (gotify_core::model::NewClient, GotifyClient, i64) {
    let new = create_client(base, "admin", "admin", "Ubuntu tray (app-test)").await.unwrap();
    let admin = GotifyClient::new(base.clone(), new.token.clone());
    let newest = admin.messages_page(None, None).await.unwrap().messages.first().map_or(0, |m| m.id);
    (new, admin, newest)
}

async fn start(base: &url::Url, last_notified: i64, sync_delay: Duration) -> Running {
    let (new, admin, _) = setup(base).await;
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::in_dir(dir.path());
    save_config(&paths, &Config { server_url: base.clone(), client_id: Some(new.id), client_name: Some(new.name.clone()), user_name: "admin".into() })
        .unwrap();
    save_state(&paths, &State { last_read_id: Some(last_notified), last_notified_id: Some(last_notified), ..State::default() })
        .unwrap();
    let sent = Recorder::default();
    let token = new.token.clone();
    let deps = AppDeps {
        paths,
        notifier: Notifier::new(sent.clone()),
        tokens: Box::new(move |_| {
            let token = token.clone();
            Box::pin(async move { Ok(Some(token)) })
        }),
        launcher: Box::new(NoLauncher),
        last_read: Arc::new(AtomicI64::new(0)),
        sync_delay,
    };
    let (app, tx, view) = App::new(deps);
    tokio::spawn(app.run());
    Running { tx, view, sent, admin, client_id: new.id, _dir: dir }
}

async fn wait_for(view: &mut watch::Receiver<TrayView>, what: impl Fn(&TrayView) -> bool) {
    tokio::time::timeout(Duration::from_secs(10), view.wait_for(|v| what(v))).await.expect("timed out").unwrap();
}

#[tokio::test]
async fn missed_messages_summarised() {
    let Some(base) = base() else { return };
    let _guard = SERIAL.lock().await;
    let (_, _, newest) = setup(&base).await;
    for i in 0..3 {
        send_as_kuma(&base, &format!("missed {i}"), 5).await;
    }
    let mut r = start(&base, newest, Duration::ZERO).await;
    wait_for(&mut r.view, |v| v.status_line.starts_with("Connected")).await;
    assert_eq!(r.sent.summaries(), ["3 missed messages"]);
}

#[tokio::test]
async fn sync_buffers_stream_messages() {
    let Some(base) = base() else { return };
    let _guard = SERIAL.lock().await;
    let (_, _, newest) = setup(&base).await;
    // Missed while the app wasn't running…
    send_as_kuma(&base, "[buffer-old] [🔴 Down] while offline", 8).await;
    let mut r = start(&base, newest, Duration::from_millis(1500)).await;
    tokio::time::sleep(Duration::from_millis(700)).await; // stream open, sync still waiting
    // …and a newer one arriving live during the sync: neither may be lost or doubled.
    send_as_kuma(&base, "[buffer-new] [🔴 Down] during sync", 8).await;
    wait_for(&mut r.view, |v| v.status_line.starts_with("Connected")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(r.sent.summaries(), ["2 missed messages"]);
}

#[tokio::test]
async fn unauthorized_stops_reconnecting() {
    let Some(base) = base() else { return };
    let _guard = SERIAL.lock().await;
    let (_, _, newest) = setup(&base).await;
    let mut r = start(&base, newest, Duration::ZERO).await;
    wait_for(&mut r.view, |v| v.status_line.starts_with("Connected")).await;
    // Revoke this app's client the way the web UI would (Basic auth is elevated).
    reqwest::Client::new()
        .delete(endpoint(&base, &format!("client/{}", r.client_id)))
        .basic_auth("admin", Some("admin"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    r.tx.send(Event::Service(ServiceCall::Resync)).unwrap();
    wait_for(&mut r.view, |v| v.status_line == "Login expired — open Settings").await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(r.view.borrow().status_line, "Login expired — open Settings");
    assert_eq!(r.sent.summaries(), ["Gotify login expired"]);
}

#[tokio::test]
async fn deleting_down_message_clears_on_resync() {
    let Some(base) = base() else { return };
    let _guard = SERIAL.lock().await;
    let (_, _, newest) = setup(&base).await;
    let mut r = start(&base, newest, Duration::ZERO).await;
    wait_for(&mut r.view, |v| v.status_line.starts_with("Connected")).await;
    let id = send_as_kuma(&base, "[delete-mon] [🔴 Down] x", 8).await;
    wait_for(&mut r.view, |v| v.down.iter().any(|d| d.monitor == "delete-mon")).await;
    r.admin.delete_message(id).await.unwrap();
    r.tx.send(Event::Service(ServiceCall::Resync)).unwrap();
    wait_for(&mut r.view, |v| !v.down.iter().any(|d| d.monitor == "delete-mon")).await;
}
