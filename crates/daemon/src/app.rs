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
use crate::tray::{DownEntry, IconKind, MonitorLine, MutedEntry, TrayCommand, TrayView};
use gotify_core::kuma::{KumaStatus, parse_kuma};
use gotify_core::kuma_status::{KumaSnapshot, KumaState, StatusPage, fetch_snapshot};
use gotify_core::mutes::{MuteFor, MuteTarget, Mutes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conn {
    NotConfigured,
    WaitingForKeyring,
    LoginExpired,
    Connecting,
    Connected,
    Reconnecting,
}

#[cfg(test)]
pub fn tray_view(conn: &Conn, server_host: Option<&str>, monitors: &Monitors, unread: usize) -> TrayView {
    tray_view_at(conn, server_host, monitors, unread, &Mutes::default(), &HashMap::new(), chrono::Utc::now(), &KumaView::Off)
}

/// What the Uptime Kuma status page says, as far as the tray is concerned.
#[derive(Debug, Clone, Copy)]
pub enum KumaView<'a> {
    /// No status page configured: outages come from Gotify messages.
    Off,
    /// Configured, first answer not in yet.
    Waiting,
    Ok(&'a KumaSnapshot),
    /// Repeatedly unreachable; `last` is the last good answer.
    Failing { last: Option<&'a KumaSnapshot> },
}

fn with_newer_messages(snapshot: &KumaSnapshot, monitors: &Monitors) -> KumaSnapshot {
    let mut effective = snapshot.clone();
    let Some(fetched_at) = snapshot.fetched_at else { return effective };
    for m in &mut effective.monitors {
        if let Some(r) = monitors.get(&m.name).filter(|r| r.since > fetched_at) {
            m.state = match r.status {
                KumaStatus::Down => KumaState::Down,
                KumaStatus::Up => KumaState::Up,
            };
            m.since = Some(r.since);
        }
    }
    effective
}

fn kuma_status_line(snapshot: &KumaSnapshot) -> String {
    let c = snapshot.counts();
    let mut line = if c.down > 0 {
        format!("{} of {} monitors down", c.down, c.total)
    } else {
        format!("{} monitors up", c.up)
    };
    if c.maintenance > 0 {
        line.push_str(&format!(", {} in maintenance", c.maintenance));
    }
    line
}

#[allow(clippy::too_many_arguments)]
pub fn tray_view_at(
    conn: &Conn,
    server_host: Option<&str>,
    monitors: &Monitors,
    unread: usize,
    mutes: &Mutes,
    app_names: &HashMap<i64, String>,
    now: chrono::DateTime<chrono::Utc>,
    kuma: &KumaView,
) -> TrayView {
    let snapshot = match kuma {
        KumaView::Ok(s) => Some(*s),
        KumaView::Failing { last } => *last,
        KumaView::Off | KumaView::Waiting => None,
    };
    // A Kuma message from Gotify that is newer than the status page's answer
    // wins for its monitor, so icon and list change together with the pop-up.
    let snapshot = snapshot.map(|s| with_newer_messages(s, monitors));
    let snapshot = snapshot.as_ref();
    let status_line = match conn {
        Conn::Connected => {
            let host = server_host.unwrap_or("Gotify");
            match kuma {
                KumaView::Off => format!("Connected to {host}"),
                KumaView::Waiting => "Connected — checking Uptime Kuma…".into(),
                KumaView::Failing { .. } => "Connected — can't reach Uptime Kuma".into(),
                KumaView::Ok(_) => format!("Connected — {}", kuma_status_line(snapshot.expect("Ok has a snapshot"))),
            }
        }
        Conn::Reconnecting => "Reconnecting…".into(),
        Conn::Connecting => "Connecting…".into(),
        Conn::NotConfigured => "Not configured — open Settings".into(),
        Conn::WaitingForKeyring => "Waiting for keyring".into(),
        Conn::LoginExpired => "Login expired — open Settings".into(),
    };
    // Without a working login the old "down" list may be stale; don't show it.
    let online = matches!(conn, Conn::Connected | Conn::Reconnecting | Conn::Connecting);
    let monitor_muted = |name: &str| mutes.is_target_muted(&MuteTarget::Monitor(name.to_string()), now);
    // The Gotify Down message for a monitor, if history has one.
    let message_of = |name: &str| monitors.get(name).filter(|r| r.status == KumaStatus::Down).map(|r| r.message_id);
    // Current outages: from Kuma when configured, else inferred from messages.
    let outages: Vec<DownEntry> = match (kuma, snapshot) {
        (KumaView::Off, _) => down_list(monitors)
            .into_iter()
            .map(|(name, r)| DownEntry { monitor: name.to_string(), message_id: Some(r.message_id), since: r.since, clearable: true })
            .collect(),
        (_, Some(s)) => s
            .monitors
            .iter()
            .filter(|m| m.state == KumaState::Down)
            .map(|m| DownEntry {
                monitor: m.name.clone(),
                message_id: message_of(&m.name),
                since: m.since.unwrap_or(now),
                clearable: false,
            })
            .collect(),
        (_, None) => Vec::new(),
    };
    let down: Vec<DownEntry> = outages.iter().filter(|d| online && !monitor_muted(&d.monitor)).cloned().collect();
    let muted: Vec<MutedEntry> = if online {
        mutes
            .list(now)
            .into_iter()
            .map(|(target, until)| {
                let (name, down) = match &target {
                    MuteTarget::Monitor(name) => (
                        name.clone(),
                        outages.iter().find(|d| d.monitor == *name).map(|d| (d.message_id, d.since)),
                    ),
                    MuteTarget::App(id) => (app_names.get(id).cloned().unwrap_or_else(|| format!("App {id}")), None),
                };
                MutedEntry { target, name, down, until }
            })
            .collect()
    } else {
        Vec::new()
    };
    // A known outage stays visible while reconnecting.
    let icon = match conn {
        _ if !down.is_empty() && online => IconKind::Down,
        Conn::Connected if matches!(kuma, KumaView::Failing { .. }) => IconKind::Unsure,
        Conn::Connected => IconKind::Ok,
        _ => IconKind::Offline,
    };
    let all = if online {
        snapshot
            .map(|s| {
                s.monitors
                    .iter()
                    .map(|m| MonitorLine { name: m.name.clone(), state: m.state, since: m.since, uptime_24h: m.uptime_24h })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    TrayView { icon, unread: unread > 0, status_line, down, muted, all }
}

/// The 15-minute re-read of the Gotify history is only needed when outages are
/// worked out from messages; with a status page, Kuma decides (saves data).
pub fn periodic_gotify_resync(status_page_configured: bool) -> bool {
    !status_page_configured
}

/// Monitors whose mute just ended while still down, as messages to show
/// again (so a real outage can't hide behind a forgotten mute).
pub fn still_down_after_mute(ended: &[MuteTarget], monitors: &Monitors) -> Vec<Message> {
    ended
        .iter()
        .filter_map(|target| match target {
            MuteTarget::Monitor(name) => monitors.get(name).filter(|r| r.status == KumaStatus::Down).map(|r| (name, r)),
            MuteTarget::App(_) => None,
        })
        .map(|(name, r)| Message {
            id: r.message_id,
            appid: 0,
            message: format!("[{name}] [🔴 Down] Still down — the mute has ended."),
            title: None,
            priority: 8,
            date: r.since,
            extras: None,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceCall {
    ShowWindow(Option<i64>),
    Resync,
    ReloadConfig,
    MarkRead(i64),
    /// None = unmute.
    SetMute(MuteTarget, Option<MuteFor>),
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
    /// The next mute end time may have passed.
    MuteCheck(u64),
    /// Time to ask the Uptime Kuma status page again.
    KumaPoll(u64),
    KumaAnswer(u64, Result<KumaSnapshot, String>),
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
    /// Shared with the D-Bus service's `Mutes`.
    pub shared_mutes: Arc<std::sync::Mutex<Mutes>>,
    /// How often to ask the Uptime Kuma status page.
    pub kuma_poll_every: Duration,
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
    mute_seq: u64,
    kuma: Kuma,
}

/// Uptime Kuma status page polling state.
#[derive(Default)]
struct Kuma {
    page: Option<StatusPage>,
    snapshot: Option<KumaSnapshot>,
    failures: u32,
    seq: u64,
    last_poll: Option<Instant>,
}

/// After a Kuma message, ask the status page again once its 60 s cache has
/// expired, to confirm the new state.
const KUMA_CONFIRM_AFTER: Duration = Duration::from_secs(65);

/// Failed polls in a row before the tray turns amber (one may be a blip).
const KUMA_FAILURES_FOR_AMBER: u32 = 2;

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
            mute_seq: 0,
            kuma: Kuma::default(),
        };
        (app, tx, view_rx)
    }

    pub async fn run(mut self) {
        self.deps.last_read.store(self.state.last_read_id.unwrap_or(0), Ordering::Relaxed);
        *self.deps.shared_mutes.lock().expect("mutes lock") = self.state.mutes.clone();
        self.schedule_mute_check();
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
        let view = tray_view_at(
            &self.conn,
            host.as_deref(),
            &self.state.monitors,
            unread,
            &self.state.mutes,
            &self.apps,
            chrono::Utc::now(),
            &self.kuma_view(),
        );
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
        *self.deps.shared_mutes.lock().expect("mutes lock") = self.state.mutes.clone();
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
        self.restart_kuma();
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
        let (mutes, now) = (self.state.mutes.clone(), chrono::Utc::now());
        let (notice, last) = missed_notice(self.state.last_notified_id, &result.messages, |id| apps.get(&id).cloned(), |m| mutes.is_muted(m, now));
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

    fn kuma_view(&self) -> KumaView<'_> {
        match (&self.kuma.page, &self.kuma.snapshot) {
            (None, _) => KumaView::Off,
            _ if self.kuma.failures >= KUMA_FAILURES_FOR_AMBER => KumaView::Failing { last: self.kuma.snapshot.as_ref() },
            (_, Some(s)) => KumaView::Ok(s),
            (_, None) => KumaView::Waiting,
        }
    }

    /// (Re)reads the status page address from the config and asks right away.
    fn restart_kuma(&mut self) {
        let page = self.config.as_ref().and_then(|c| c.kuma_status_page.as_deref()).and_then(|u| StatusPage::parse(u).ok());
        if page != self.kuma.page {
            self.kuma = Kuma { page, seq: self.kuma.seq, ..Kuma::default() };
        }
        self.poll_kuma_now();
    }

    fn poll_kuma_now(&mut self) {
        self.kuma.seq += 1;
        if self.kuma.page.is_some() {
            self.schedule(Duration::ZERO, Event::KumaPoll(self.kuma.seq));
        }
    }

    fn on_kuma_poll(&mut self) {
        let Some(page) = self.kuma.page.clone() else { return };
        self.kuma.last_poll = Some(Instant::now());
        let (tx, seq) = (self.tx.clone(), self.kuma.seq);
        tokio::spawn(async move {
            let _ = tx.send(Event::KumaAnswer(seq, fetch_snapshot(&page).await));
        });
    }

    fn on_kuma_answer(&mut self, answer: Result<KumaSnapshot, String>) {
        match answer {
            Ok(snapshot) => {
                self.kuma.snapshot = Some(snapshot);
                self.kuma.failures = 0;
            }
            Err(e) => {
                if self.kuma.failures == 0 {
                    eprintln!("gotify-tray: Uptime Kuma status page: {e}");
                }
                self.kuma.failures += 1;
            }
        }
        self.schedule(self.deps.kuma_poll_every, Event::KumaPoll(self.kuma.seq));
    }

    fn set_mute(&mut self, target: MuteTarget, duration: Option<MuteFor>) {
        match duration {
            Some(d) => self.state.mutes.mute(target, d, chrono::Utc::now()),
            None => self.state.mutes.unmute(&target),
        }
        self.save();
        self.schedule_mute_check();
    }

    /// Wakes up when the next mute ends (recomputed from the wall clock, so
    /// suspend doesn't delay it: Wake and Tick call this again).
    fn schedule_mute_check(&mut self) {
        self.mute_seq += 1;
        if let Some(end) = self.state.mutes.next_expiry() {
            let delay = (end - chrono::Utc::now()).to_std().unwrap_or(Duration::ZERO);
            self.schedule(delay, Event::MuteCheck(self.mute_seq));
        }
    }

    async fn on_mute_check(&mut self) {
        let ended = self.state.mutes.take_expired(chrono::Utc::now());
        if !ended.is_empty() {
            self.save();
            for msg in still_down_after_mute(&ended, &self.state.monitors) {
                self.deps.notifier.show_message(&msg, None).await;
            }
        }
        self.schedule_mute_check();
    }

    async fn on_live(&mut self, msg: Message) {
        apply_live(&mut self.state.monitors, &msg);
        if self.kuma.page.is_some() && parse_kuma(&msg.message).is_some() {
            // Restart the polling cycle with a confirming check after Kuma's cache.
            self.kuma.seq += 1;
            self.schedule(KUMA_CONFIRM_AFTER, Event::KumaPoll(self.kuma.seq));
        }
        self.message_ids.insert(msg.id);
        if self.state.last_notified_id.is_some_and(|last| msg.id > last) {
            if !self.state.mutes.is_muted(&msg, chrono::Utc::now()) {
                let app = self.apps.get(&msg.appid).cloned();
                self.deps.notifier.show_message(&msg, app.as_deref()).await;
            }
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
                self.schedule_mute_check();
                self.poll_kuma_now();
                // Time asleep doesn't count towards the outage alert.
                if let Some(seq) = self.outage.restart_clock() {
                    self.schedule(self.deps.outage_alert_after, Event::OutageCheck(seq));
                }
                if matches!(self.conn, Conn::Connected | Conn::Connecting | Conn::Reconnecting) {
                    self.backoff.reset();
                    self.connect();
                }
            }
            Event::Tick => {
                self.schedule_mute_check();
                if periodic_gotify_resync(self.kuma.page.is_some()) {
                    self.resync_now();
                }
            }
            Event::MuteCheck(seq) if seq == self.mute_seq => self.on_mute_check().await,
            Event::MuteCheck(_) => {}
            Event::KumaPoll(seq) if seq == self.kuma.seq => self.on_kuma_poll(),
            Event::KumaAnswer(seq, answer) if seq == self.kuma.seq => self.on_kuma_answer(answer),
            Event::KumaPoll(_) | Event::KumaAnswer(..) => {}
            Event::WindowExited => self.resync_now(),
            Event::Tray(cmd) => match cmd {
                TrayCommand::MenuOpened => {
                    // Kuma caches its answer for a minute; asking more often gains nothing.
                    if self.kuma.last_poll.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                        self.poll_kuma_now();
                    }
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
                TrayCommand::Mute(target, duration) => self.set_mute(target, Some(duration)),
                TrayCommand::Unmute(target) => self.set_mute(target, None),
                TrayCommand::OpenStatusPage => {
                    if let Some(page) = &self.kuma.page {
                        self.deps.launcher.open_link(page.display_url().to_string());
                    }
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
                ServiceCall::SetMute(target, duration) => self.set_mute(target, duration),
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
    use gotify_core::kuma_status::KumaMonitor;
    use gotify_core::mutes::{MuteFor, MuteTarget, Mutes};

    fn monitors_with_down() -> Monitors {
        Monitors::from([(
            "web01".to_string(),
            MonitorRecord { status: KumaStatus::Down, message_id: 4, since: Utc.timestamp_opt(60, 0).unwrap() },
        )])
    }

    #[test]
    fn ended_mutes_of_monitors_still_down_are_announced_again() {
        let mut monitors = monitors_with_down(); // web01 down (message 4)
        monitors.insert(
            "db".into(),
            MonitorRecord { status: KumaStatus::Up, message_id: 5, since: Utc.timestamp_opt(60, 0).unwrap() },
        );
        let ended = [MuteTarget::Monitor("web01".into()), MuteTarget::Monitor("db".into()), MuteTarget::App(3)];
        let again = still_down_after_mute(&ended, &monitors);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].id, 4);
        assert_eq!(again[0].message, "[web01] [🔴 Down] Still down — the mute has ended.");
    }

    #[test]
    fn muted_outage_is_not_red_and_listed_as_muted() {
        let now = Utc.timestamp_opt(120, 0).unwrap();
        let mut mutes = Mutes::default();
        mutes.mute(MuteTarget::Monitor("web01".into()), MuteFor::Day, now);
        let v = tray_view_at(&Conn::Connected, Some("h"), &monitors_with_down(), 0, &mutes, &HashMap::new(), now, &KumaView::Off);
        assert_eq!(v.icon, IconKind::Ok);
        assert!(v.down.is_empty());
        assert_eq!(v.muted.len(), 1);
        assert_eq!(v.muted[0].name, "web01");
        assert_eq!(v.muted[0].down.map(|(id, _)| id), Some(Some(4)));
    }

    fn kuma(states: &[(&str, KumaState)]) -> KumaSnapshot {
        KumaSnapshot {
            monitors: states
                .iter()
                .map(|(n, st)| KumaMonitor {
                    name: n.to_string(),
                    state: *st,
                    since: Some(Utc.timestamp_opt(600, 0).unwrap()),
                    message: String::new(),
                    uptime_24h: None,
                })
                .collect(),
            fetched_at: None,
        }
    }

    fn view_with(kuma_view: KumaView, monitors: &Monitors) -> TrayView {
        tray_view_at(&Conn::Connected, Some("h"), monitors, 0, &Mutes::default(), &HashMap::new(), Utc::now(), &kuma_view)
    }

    #[test]
    fn kuma_decides_what_is_down() {
        let snap = kuma(&[("web01", KumaState::Up), ("db", KumaState::Down), ("backup", KumaState::Maintenance)]);
        // History still thinks web01 is down; Kuma says it's up.
        let v = view_with(KumaView::Ok(&snap), &monitors_with_down());
        assert_eq!(v.icon, IconKind::Down);
        assert_eq!(v.down.iter().map(|d| d.monitor.as_str()).collect::<Vec<_>>(), ["db"]);
        assert_eq!(v.down[0].message_id, None, "no Gotify message for db");
        assert!(!v.down[0].clearable);
        assert_eq!(v.status_line, "Connected — 1 of 3 monitors down, 1 in maintenance");
    }

    #[test]
    fn all_monitors_come_from_the_status_page() {
        let snap = kuma(&[("web01", KumaState::Up), ("db", KumaState::Down)]);
        let v = view_with(KumaView::Ok(&snap), &Monitors::new());
        assert_eq!(v.all.iter().map(|m| (m.name.as_str(), m.state)).collect::<Vec<_>>(), [("web01", KumaState::Up), ("db", KumaState::Down)]);
        assert!(view_with(KumaView::Off, &Monitors::new()).all.is_empty());
    }

    #[test]
    fn newer_gotify_message_wins_until_the_next_status_page_answer() {
        // Status page (fetched at t=600) says web01 is up…
        let mut snap = kuma(&[("web01", KumaState::Up), ("db", KumaState::Up)]);
        snap.fetched_at = Some(Utc.timestamp_opt(600, 0).unwrap());
        // …then Gotify delivers "web01 down" at t=900.
        let mut monitors = Monitors::new();
        monitors.insert(
            "web01".into(),
            MonitorRecord { status: KumaStatus::Down, message_id: 9, since: Utc.timestamp_opt(900, 0).unwrap() },
        );
        let v = view_with(KumaView::Ok(&snap), &monitors);
        assert_eq!(v.icon, IconKind::Down);
        assert_eq!(v.down[0].monitor, "web01");
        assert_eq!(v.down[0].message_id, Some(9));
        assert_eq!(v.status_line, "Connected — 1 of 2 monitors down");
        assert_eq!(v.all[0].state, KumaState::Down, "the list agrees with the icon");

        // An older message doesn't override a newer status page answer.
        snap.fetched_at = Some(Utc.timestamp_opt(1000, 0).unwrap());
        let v = view_with(KumaView::Ok(&snap), &monitors);
        assert_eq!(v.icon, IconKind::Ok);
    }

    #[test]
    fn periodic_gotify_recheck_only_without_status_page() {
        // With a status page, Kuma decides what's down; re-reading the whole
        // Gotify history every 15 minutes would only cost data.
        assert!(periodic_gotify_resync(false));
        assert!(!periodic_gotify_resync(true));
    }

    #[test]
    fn kuma_all_up_is_green_with_count() {
        let snap = kuma(&[("web01", KumaState::Up), ("db", KumaState::Up)]);
        let v = view_with(KumaView::Ok(&snap), &Monitors::new());
        assert_eq!((v.icon, v.status_line.as_str()), (IconKind::Ok, "Connected — 2 monitors up"));
    }

    #[test]
    fn down_entry_links_to_the_gotify_message_when_there_is_one() {
        let snap = kuma(&[("web01", KumaState::Down)]);
        let v = view_with(KumaView::Ok(&snap), &monitors_with_down());
        assert_eq!(v.down[0].message_id, Some(4));
    }

    #[test]
    fn unreachable_kuma_is_amber_unless_something_is_known_down() {
        let v = view_with(KumaView::Failing { last: None }, &Monitors::new());
        assert_eq!((v.icon, v.status_line.as_str()), (IconKind::Unsure, "Connected — can't reach Uptime Kuma"));
        let snap = kuma(&[("db", KumaState::Down)]);
        let v = view_with(KumaView::Failing { last: Some(&snap) }, &Monitors::new());
        assert_eq!(v.icon, IconKind::Down, "a known outage still shows red");
        let v = view_with(KumaView::Waiting, &Monitors::new());
        assert_eq!((v.icon, v.status_line.as_str()), (IconKind::Ok, "Connected — checking Uptime Kuma…"));
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

        for conn in [Conn::NotConfigured, Conn::LoginExpired, Conn::WaitingForKeyring] {
            assert!(tray_view(&conn, None, &monitors_with_down(), 0).down.is_empty(), "{conn:?} shows stale outages");
        }

        let v = tray_view(&Conn::NotConfigured, None, &Monitors::new(), 0);
        assert_eq!((v.icon, v.status_line.as_str()), (IconKind::Offline, "Not configured — open Settings"));
        assert_eq!(tray_view(&Conn::LoginExpired, None, &Monitors::new(), 0).status_line, "Login expired — open Settings");
        assert_eq!(tray_view(&Conn::WaitingForKeyring, None, &Monitors::new(), 0).status_line, "Waiting for keyring");
        assert_eq!(tray_view(&Conn::Connecting, None, &Monitors::new(), 0).status_line, "Connecting…");
    }
}
