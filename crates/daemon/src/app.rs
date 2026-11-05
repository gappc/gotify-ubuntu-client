//! The background service: connection state machine and event loop.

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use gotify_core::api::{ApiError, GotifyClient};
use gotify_core::consts::{KEYRING_RETRY, MENU_RESYNC_DEBOUNCE, STREAM_SILENCE_TIMEOUT};
use gotify_core::files::{Config, Paths, State, load_config, load_state, save_state};
use gotify_core::inbox::{MissedNotice, markers_stale, missed_notice, unread_count};
use gotify_core::model::{Application, Message};
use gotify_core::monitors::{self, Monitors, apply_live, down_list, resync};
use gotify_core::secret::SecretError;
use gotify_core::stream::{StreamEnd, connect_stream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use url::Url;

use crate::backoff::Backoff;
use crate::outage::Outage;
use crate::notify::{NotifierSignal, NotifyBackend, Notifier, Target};
use crate::tray::{DownEntry, IconKind, TrayCommand, TrayView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conn {
    NotConfigured,
    WaitingForKeyring,
    LoginExpired,
    Connecting,
    Connected,
    Reconnecting,
}

pub fn tray_view(conn: &Conn, server_host: Option<&str>, monitors: &Monitors, unread: usize) -> TrayView {
    let status_line = match conn {
        Conn::Connected => format!("Connected to {}", server_host.unwrap_or("Gotify")),
        Conn::Reconnecting => "Reconnecting…".into(),
        Conn::Connecting => "Connecting…".into(),
        Conn::NotConfigured => "Not configured — open Settings".into(),
        Conn::WaitingForKeyring => "Waiting for keyring".into(),
        Conn::LoginExpired => "Login expired — open Settings".into(),
    };
    let down: Vec<DownEntry> = down_list(monitors)
        .into_iter()
        .map(|(name, r)| DownEntry { monitor: name.to_string(), message_id: r.message_id, since: r.since })
        .collect();
    // A known outage stays visible while reconnecting.
    let icon = match conn {
        _ if !down.is_empty() && matches!(conn, Conn::Connected | Conn::Reconnecting | Conn::Connecting) => IconKind::Down,
        Conn::Connected => IconKind::Ok,
        _ => IconKind::Offline,
    };
    TrayView { icon, unread: unread > 0, status_line, down }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceCall {
    ShowWindow(Option<i64>),
    Resync,
    ReloadConfig,
    MarkRead(i64),
}

pub struct SyncResult {
    pub apps: Vec<Application>,
    pub messages: Vec<Message>,
}

/// Everything that can happen. Events from background tasks carry the
/// connection generation so results of an abandoned connection are ignored.
pub enum Event {
    StreamOpened(u64),
    Stream(u64, Message),
    StreamEnded(u64, StreamEnd),
    Synced(u64, Result<SyncResult, ApiError>),
    Reconnect(u64),
    KeyringRetry(u64),
    Tray(TrayCommand),
    Notifier(NotifierSignal),
    Service(ServiceCall),
    WindowExited,
    /// Resume from suspend or network back.
    Wake,
    /// Periodic re-sync.
    Tick,
    /// The outage alert delay for this outage has passed.
    OutageCheck(u64),
}

/// Starts the window process or a browser.
pub trait Launcher: Send {
    /// The started process, if any; its exit triggers a re-sync.
    fn window(&mut self, args: Vec<String>) -> Option<tokio::process::Child>;
    fn open_link(&mut self, url: String);
}

pub type TokenFuture = Pin<Box<dyn Future<Output = Result<Option<String>, SecretError>> + Send>>;
pub type TokenSource = Box<dyn Fn(Url) -> TokenFuture + Send>;

pub struct AppDeps<B: NotifyBackend> {
    pub paths: Paths,
    pub notifier: Notifier<B>,
    pub tokens: TokenSource,
    pub launcher: Box<dyn Launcher>,
    /// Shared with the D-Bus service's `LastRead`.
    pub last_read: Arc<AtomicI64>,
    /// Test hook: wait this long before fetching during a sync.
    pub sync_delay: Duration,
    /// How long the connection may be down before "Can't reach Gotify".
    pub outage_alert_after: Duration,
}

pub struct App<B: NotifyBackend> {
    deps: AppDeps<B>,
    tx: mpsc::UnboundedSender<Event>,
    rx: mpsc::UnboundedReceiver<Event>,
    view: watch::Sender<TrayView>,
    state: State,
    config: Option<Config>,
    client: Option<GotifyClient>,
    conn: Conn,
    generation: u64,
    stream_task: Option<JoinHandle<()>>,
    backoff: Backoff,
    apps: HashMap<i64, String>,
    message_ids: BTreeSet<i64>,
    syncing: bool,
    /// Stream messages received while a sync is running.
    pending: Vec<Message>,
    last_sync: Option<Instant>,
    login_expired_notified: bool,
    outage: Outage,
}

fn args_for_message(id: Option<i64>) -> Vec<String> {
    id.map_or_else(Vec::new, |id| vec!["--message".into(), id.to_string()])
}

impl<B: NotifyBackend> App<B> {
    pub fn new(deps: AppDeps<B>) -> (Self, mpsc::UnboundedSender<Event>, watch::Receiver<TrayView>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (view, view_rx) = watch::channel(TrayView::default());
        let state = load_state(&deps.paths);
        let app = App {
            deps,
            tx: tx.clone(),
            rx,
            view,
            state,
            config: None,
            client: None,
            conn: Conn::NotConfigured,
            generation: 0,
            stream_task: None,
            backoff: Backoff::default(),
            apps: HashMap::new(),
            message_ids: BTreeSet::new(),
            syncing: false,
            pending: Vec::new(),
            last_sync: None,
            login_expired_notified: false,
            outage: Outage::default(),
        };
        (app, tx, view_rx)
    }

    pub async fn run(mut self) {
        self.deps.last_read.store(self.state.last_read_id.unwrap_or(0), Ordering::Relaxed);
        self.start().await;
        while let Some(event) = self.rx.recv().await {
            if !self.handle(event).await {
                break;
            }
            self.publish();
        }
        self.stop_connection();
    }

    fn publish(&self) {
        let host = self.config.as_ref().and_then(|c| c.server_url.host_str().map(str::to_string));
        let unread = unread_count(self.state.last_read_id, self.message_ids.iter().copied());
        let view = tray_view(&self.conn, host.as_deref(), &self.state.monitors, unread);
        self.view.send_if_modified(|old| {
            let changed = *old != view;
            *old = view;
            changed
        });
    }

    fn save(&self) {
        if let Err(e) = save_state(&self.deps.paths, &self.state) {
            eprintln!("gotify-tray: can't save state: {e}");
        }
        self.deps.last_read.store(self.state.last_read_id.unwrap_or(0), Ordering::Relaxed);
    }

    fn stop_connection(&mut self) {
        self.generation += 1;
        if let Some(task) = self.stream_task.take() {
            task.abort();
        }
        self.syncing = false;
        self.pending.clear();
    }

    fn schedule(&self, after: Duration, event: Event) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = tx.send(event);
        });
    }

    /// (Re)start from config and keyring.
    async fn start(&mut self) {
        // Logged out, new login or keyring retry: not a server outage.
        if self.outage.cancel() {
            self.deps.notifier.close_outage().await;
        }
        self.stop_connection();
        self.client = None;
        self.config = match load_config(&self.deps.paths) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("gotify-tray: can't read config: {e}");
                None
            }
        };
        let Some(config) = self.config.clone() else {
            self.conn = Conn::NotConfigured;
            return;
        };
        if self.state.bind_to_server(&config.server_url) {
            self.save();
        }
        match (self.deps.tokens)(config.server_url.clone()).await {
            Ok(Some(token)) => {
                self.client = Some(GotifyClient::new(config.server_url, token));
                self.connect();
            }
            Ok(None) => self.conn = Conn::NotConfigured,
            Err(e) => {
                if let SecretError::Unavailable(why) = &e {
                    eprintln!("gotify-tray: keyring: {why}");
                }
                self.conn = Conn::WaitingForKeyring;
                self.schedule(KEYRING_RETRY, Event::KeyringRetry(self.generation));
            }
        }
    }

    fn connect(&mut self) {
        let Some(client) = self.client.clone() else { return };
        self.stop_connection();
        if self.conn != Conn::Reconnecting {
            self.conn = Conn::Connecting;
        }
        let generation = self.generation;
        let tx = self.tx.clone();
        self.stream_task = Some(tokio::spawn(async move {
            let mut stream = match connect_stream(client.base(), client.token(), STREAM_SILENCE_TIMEOUT).await {
                Ok(s) => s,
                Err(ApiError::Unauthorized) => {
                    let _ = tx.send(Event::StreamEnded(generation, StreamEnd::Unauthorized));
                    return;
                }
                Err(e) => {
                    let _ = tx.send(Event::StreamEnded(generation, StreamEnd::Error(e.to_string())));
                    return;
                }
            };
            let _ = tx.send(Event::StreamOpened(generation));
            loop {
                match stream.next().await {
                    Ok(msg) => {
                        let _ = tx.send(Event::Stream(generation, msg));
                    }
                    Err(end) => {
                        let _ = tx.send(Event::StreamEnded(generation, end));
                        return;
                    }
                }
            }
        }));
    }

    fn start_sync(&mut self) {
        let Some(client) = self.client.clone() else { return };
        if self.syncing {
            return;
        }
        self.syncing = true;
        let generation = self.generation;
        let delay = self.deps.sync_delay;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let result = async { Ok(SyncResult { apps: client.applications().await?, messages: client.all_messages().await? }) };
            let _ = tx.send(Event::Synced(generation, result.await));
        });
    }

    fn resync_now(&mut self) {
        if self.conn == Conn::Connected {
            self.start_sync();
        }
    }

    fn reconnect_later(&mut self, reason: &str) {
        if let Some(seq) = self.outage.disconnected(reason) {
            self.schedule(self.deps.outage_alert_after, Event::OutageCheck(seq));
        }
        self.stop_connection();
        self.conn = Conn::Reconnecting;
        let delay = self.backoff.next_delay();
        self.schedule(delay, Event::Reconnect(self.generation));
    }

    async fn login_expired(&mut self) {
        if self.outage.cancel() {
            self.deps.notifier.close_outage().await;
        }
        self.stop_connection();
        self.conn = Conn::LoginExpired;
        if !self.login_expired_notified {
            self.login_expired_notified = true;
            self.deps.notifier.show_login_expired().await;
        }
    }

    async fn on_synced(&mut self, result: SyncResult) {
        self.syncing = false;
        self.last_sync = Some(Instant::now());
        self.backoff.reset();
        self.conn = Conn::Connected;
        if self.outage.connected() {
            self.deps.notifier.show_reconnected().await;
        }
        self.apps = result.apps.into_iter().map(|a| (a.id, a.name)).collect();
        self.message_ids = result.messages.iter().map(|m| m.id).collect();
        self.state.monitors = resync(&self.state.monitors, &result.messages);

        let newest = result.messages.iter().map(|m| m.id).max();
        if markers_stale(self.state.last_notified_id, newest) {
            eprintln!("gotify-tray: server message ids went backwards; starting fresh");
            self.state.last_notified_id = None;
            self.state.last_read_id = None;
        }
        let apps = self.apps.clone();
        let (notice, last) = missed_notice(self.state.last_notified_id, &result.messages, |id| apps.get(&id).cloned());
        let first_run = self.state.last_notified_id.is_none();
        if let Some(old) = self.state.last_notified_id {
            let missed: Vec<Message> = result.messages.iter().filter(|m| m.id > old).cloned().collect();
            self.deps.notifier.close_recovered(&missed).await;
        }
        self.state.last_notified_id = last;
        if first_run && self.state.last_read_id.is_none() {
            self.state.last_read_id = last;
        }
        match notice {
            MissedNotice::None => {}
            MissedNotice::Single(msg) => {
                let app = self.apps.get(&msg.appid).cloned();
                self.deps.notifier.show_message(&msg, app.as_deref()).await;
            }
            MissedNotice::Summary { count, urgency, lines } => {
                self.deps.notifier.show_summary(count, urgency, &lines).await;
            }
        }
        let mut pending = std::mem::take(&mut self.pending);
        pending.sort_by_key(|m| m.id);
        for msg in pending {
            self.on_live(msg).await;
        }
        self.save();
    }

    async fn on_live(&mut self, msg: Message) {
        apply_live(&mut self.state.monitors, &msg);
        self.message_ids.insert(msg.id);
        if self.state.last_notified_id.is_some_and(|last| msg.id > last) {
            let app = self.apps.get(&msg.appid).cloned();
            self.deps.notifier.show_message(&msg, app.as_deref()).await;
            self.state.last_notified_id = Some(msg.id);
        }
        self.save();
    }

    fn open(&mut self, target: Target) {
        let child = match target {
            Target::OpenWindow(id) => self.deps.launcher.window(args_for_message(id)),
            Target::OpenSettings => self.deps.launcher.window(vec!["--settings".into()]),
            Target::OpenLink(url) => {
                self.deps.launcher.open_link(url);
                None
            }
        };
        if let Some(mut child) = child {
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let _ = child.wait().await;
                let _ = tx.send(Event::WindowExited);
            });
        }
    }

    /// Returns false to quit.
    async fn handle(&mut self, event: Event) -> bool {
        let current = self.generation;
        match event {
            Event::StreamOpened(g) if g == current => self.start_sync(),
            Event::Stream(g, msg) if g == current => {
                if self.syncing {
                    self.pending.push(msg);
                } else {
                    self.on_live(msg).await;
                }
            }
            Event::StreamEnded(g, end) if g == current => match end {
                StreamEnd::Unauthorized => self.login_expired().await,
                StreamEnd::Closed | StreamEnd::Silent | StreamEnd::Error(_) => {
                    if !matches!(end, StreamEnd::Closed) {
                        eprintln!("gotify-tray: stream ended: {end:?}");
                    }
                    let reason = match &end {
                        StreamEnd::Silent => "The server stopped responding.".to_string(),
                        StreamEnd::Error(e) => format!("Last error: {e}"),
                        _ => "The server closed the connection.".to_string(),
                    };
                    self.reconnect_later(&reason);
                }
            },
            Event::Synced(g, result) if g == current => match result {
                Ok(r) => self.on_synced(r).await,
                Err(ApiError::Unauthorized) => self.login_expired().await,
                Err(e) => {
                    eprintln!("gotify-tray: sync failed: {e}");
                    self.syncing = false;
                    if self.conn != Conn::Connected {
                        self.reconnect_later(&format!("Last error: {e}"));
                    }
                }
            },
            Event::Reconnect(g) if g == current => self.connect(),
            Event::KeyringRetry(g) if g == current && self.conn == Conn::WaitingForKeyring => self.start().await,
            Event::StreamOpened(_)
            | Event::Stream(..)
            | Event::StreamEnded(..)
            | Event::Synced(..)
            | Event::Reconnect(_)
            | Event::KeyringRetry(_) => {}
            Event::OutageCheck(seq) => {
                if matches!(self.conn, Conn::Connecting | Conn::Reconnecting)
                    && let Some(reason) = self.outage.alert_due(seq)
                {
                    self.deps.notifier.show_outage(&reason).await;
                }
            }
            Event::Wake => {
                // Time asleep doesn't count towards the outage alert.
                if let Some(seq) = self.outage.restart_clock() {
                    self.schedule(self.deps.outage_alert_after, Event::OutageCheck(seq));
                }
                if matches!(self.conn, Conn::Connected | Conn::Connecting | Conn::Reconnecting) {
                    self.backoff.reset();
                    self.connect();
                }
            }
            Event::Tick => self.resync_now(),
            Event::WindowExited => self.resync_now(),
            Event::Tray(cmd) => match cmd {
                TrayCommand::MenuOpened => {
                    if self.last_sync.is_none_or(|t| t.elapsed() >= MENU_RESYNC_DEBOUNCE) {
                        self.resync_now();
                    }
                }
                TrayCommand::ShowWindow(id) => self.open(Target::OpenWindow(id)),
                TrayCommand::Settings => self.open(Target::OpenSettings),
                TrayCommand::ClearMonitor(name) => {
                    monitors::clear(&mut self.state.monitors, &name);
                    self.save();
                }
                TrayCommand::Quit => return false,
            },
            Event::Notifier(NotifierSignal::Action(id, key)) => {
                if let Some(target) = self.deps.notifier.on_action(id, &key) {
                    self.open(target);
                }
            }
            Event::Notifier(NotifierSignal::Closed(id)) => self.deps.notifier.on_closed(id),
            Event::Service(call) => match call {
                ServiceCall::ShowWindow(id) => self.open(Target::OpenWindow(id)),
                ServiceCall::Resync => self.resync_now(),
                ServiceCall::ReloadConfig => {
                    self.login_expired_notified = false;
                    self.backoff.reset();
                    self.start().await;
                }
                ServiceCall::MarkRead(id) => {
                    self.state.last_read_id = Some(self.state.last_read_id.map_or(id, |old| old.max(id)));
                    self.save();
                }
            },
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use gotify_core::kuma::KumaStatus;
    use gotify_core::monitors::MonitorRecord;

    fn monitors_with_down() -> Monitors {
        Monitors::from([(
            "web01".to_string(),
            MonitorRecord { status: KumaStatus::Down, message_id: 4, since: Utc.timestamp_opt(60, 0).unwrap() },
        )])
    }

    #[test]
    fn tray_view_states() {
        let v = tray_view(&Conn::Connected, Some("gotify.example.com"), &monitors_with_down(), 0);
        assert_eq!(v.icon, IconKind::Down);
        assert_eq!(v.status_line, "Connected to gotify.example.com");
        assert_eq!(v.down[0].monitor, "web01");
        assert!(!v.unread);

        let v = tray_view(&Conn::Reconnecting, Some("gotify.example.com"), &monitors_with_down(), 2);
        assert_eq!((v.icon, v.status_line.as_str(), v.unread), (IconKind::Down, "Reconnecting…", true));

        let v = tray_view(&Conn::Connected, Some("h"), &Monitors::new(), 0);
        assert_eq!(v.icon, IconKind::Ok);

        let v = tray_view(&Conn::NotConfigured, None, &Monitors::new(), 0);
        assert_eq!((v.icon, v.status_line.as_str()), (IconKind::Offline, "Not configured — open Settings"));
        assert_eq!(tray_view(&Conn::LoginExpired, None, &Monitors::new(), 0).status_line, "Login expired — open Settings");
        assert_eq!(tray_view(&Conn::WaitingForKeyring, None, &Monitors::new(), 0).status_line, "Waiting for keyring");
        assert_eq!(tray_view(&Conn::Connecting, None, &Monitors::new(), 0).status_line, "Connecting…");
    }
}
