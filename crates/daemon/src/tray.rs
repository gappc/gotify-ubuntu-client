//! Tray icon (StatusNotifierItem via ksni) and its menu.

use chrono::{DateTime, Local, Utc};
use gotify_core::kuma_status::{KumaState, uptime_label};
use gotify_core::mutes::{MuteFor, MuteTarget};
use gotify_core::text::{since_label, until_label};
use ksni::menu::{StandardItem, SubMenu};
use tokio::sync::mpsc::UnboundedSender;

pub use crate::icon::IconKind;
use crate::icon::render;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownEntry {
    pub monitor: String,
    /// The Gotify Down message, if there is one to show.
    pub message_id: Option<i64>,
    pub since: DateTime<Utc>,
    /// Only outages inferred from messages can be cleared by hand.
    pub clearable: bool,
}

/// A muted monitor or app, with its current outage (message id, since) if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutedEntry {
    pub target: MuteTarget,
    pub name: String,
    pub down: Option<(Option<i64>, DateTime<Utc>)>,
    /// None = until unmuted.
    pub until: Option<DateTime<Utc>>,
}

/// One monitor in "All monitors" (from the Kuma status page).
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorLine {
    pub name: String,
    pub state: KumaState,
    /// When the current state began (shown for outages).
    pub since: Option<DateTime<Utc>>,
    pub uptime_24h: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrayView {
    pub icon: IconKind,
    pub unread: bool,
    pub status_line: String,
    pub down: Vec<DownEntry>,
    pub muted: Vec<MutedEntry>,
    /// Every monitor on the Kuma status page (empty without one).
    pub all: Vec<MonitorLine>,
}

impl Default for TrayView {
    fn default() -> Self {
        TrayView { icon: IconKind::Offline, unread: false, status_line: "Starting…".into(), down: Vec::new(), muted: Vec::new(), all: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow(Option<i64>),
    Settings,
    ClearMonitor(String),
    Mute(MuteTarget, MuteFor),
    Unmute(MuteTarget),
    OpenStatusPage,
    MenuOpened,
    Quit,
}

/// Plain description of the menu, so its content can be tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Label(String),
    Separator,
    Item(String, TrayCommand),
    Sub(String, Vec<MenuEntry>),
}

/// "✅ api — up · 99.98 %", "🔴 web — down since 14:32 · 97.2 %".
fn monitor_line(m: &MonitorLine, now: DateTime<Local>) -> String {
    let (mark, state) = match m.state {
        KumaState::Up => ("✅", "up".to_string()),
        KumaState::Down => (
            "🔴",
            match m.since {
                Some(t) => format!("down since {}", since_label(t.with_timezone(&Local), now)),
                None => "down".into(),
            },
        ),
        KumaState::Maintenance => ("🔧", "maintenance".into()),
        KumaState::Pending => ("⏳", "pending".into()),
        KumaState::Unknown => ("❔", "no checks yet".into()),
    };
    match m.uptime_24h {
        Some(u) => format!("{mark} {} — {state} · {}", m.name, uptime_label(u)),
        None => format!("{mark} {} — {state}", m.name),
    }
}

pub fn menu_model(view: &TrayView, now: DateTime<Local>) -> Vec<MenuEntry> {
    let mut menu = vec![MenuEntry::Label(view.status_line.clone()), MenuEntry::Separator];
    if !view.down.is_empty() {
        menu.push(MenuEntry::Label("Down right now".into()));
        for d in &view.down {
            let target = MuteTarget::Monitor(d.monitor.clone());
            let mut items = Vec::new();
            if let Some(id) = d.message_id {
                items.push(MenuEntry::Item("Show message".into(), TrayCommand::ShowWindow(Some(id))));
                items.push(MenuEntry::Separator);
            }
            items.extend(
                MuteFor::ALL.map(|m| MenuEntry::Item(m.label().into(), TrayCommand::Mute(target.clone(), m))),
            );
            if d.clearable {
                items.push(MenuEntry::Separator);
                items.push(MenuEntry::Item("Clear".into(), TrayCommand::ClearMonitor(d.monitor.clone())));
            }
            menu.push(MenuEntry::Sub(
                format!("{} — down since {}", d.monitor, since_label(d.since.with_timezone(&Local), now)),
                items,
            ));
        }
        menu.push(MenuEntry::Separator);
    }
    if !view.muted.is_empty() {
        menu.push(MenuEntry::Label("Muted".into()));
        for m in &view.muted {
            let mut label = m.name.clone();
            let mut items = Vec::new();
            label.push_str(" — ");
            if let Some((id, since)) = m.down {
                label.push_str(&format!("down since {}, ", since_label(since.with_timezone(&Local), now)));
                if let Some(id) = id {
                    items.push(MenuEntry::Item("Show message".into(), TrayCommand::ShowWindow(Some(id))));
                }
            }
            match m.until {
                Some(until) => label.push_str(&format!("muted until {}", until_label(until.with_timezone(&Local), now))),
                None => label.push_str("muted"),
            }
            items.push(MenuEntry::Item("Unmute".into(), TrayCommand::Unmute(m.target.clone())));
            menu.push(MenuEntry::Sub(label, items));
        }
        menu.push(MenuEntry::Separator);
    }
    if !view.all.is_empty() {
        let mut items = vec![MenuEntry::Item("Open status page".into(), TrayCommand::OpenStatusPage), MenuEntry::Separator];
        items.extend(view.all.iter().map(|m| MenuEntry::Item(monitor_line(m, now), TrayCommand::OpenStatusPage)));
        menu.push(MenuEntry::Sub(format!("All monitors ({})", view.all.len()), items));
        menu.push(MenuEntry::Separator);
    }
    menu.extend([
        MenuEntry::Item("Show messages".into(), TrayCommand::ShowWindow(None)),
        MenuEntry::Item("Settings".into(), TrayCommand::Settings),
        MenuEntry::Item("Quit".into(), TrayCommand::Quit),
    ]);
    menu
}

pub struct GotifyTray {
    pub view: TrayView,
    tx: UnboundedSender<TrayCommand>,
}

impl GotifyTray {
    pub fn new(tx: UnboundedSender<TrayCommand>) -> Self {
        GotifyTray { view: TrayView::default(), tx }
    }
}

/// Menu labels are shown with "_x" as a keyboard mnemonic, so a literal
/// underscore must be doubled.
pub fn menu_label(text: &str) -> String {
    text.replace('_', "__")
}

fn to_ksni(entry: MenuEntry) -> ksni::MenuItem<GotifyTray> {
    match entry {
        MenuEntry::Separator => ksni::MenuItem::Separator,
        MenuEntry::Label(label) => StandardItem { label: menu_label(&label), enabled: false, ..Default::default() }.into(),
        MenuEntry::Item(label, cmd) => StandardItem {
            label: menu_label(&label),
            activate: Box::new(move |t: &mut GotifyTray| {
                let _ = t.tx.send(cmd.clone());
            }),
            ..Default::default()
        }
        .into(),
        MenuEntry::Sub(label, items) => {
            SubMenu { label: menu_label(&label), submenu: items.into_iter().map(to_ksni).collect(), ..Default::default() }
                .into()
        }
    }
}

impl ksni::Tray for GotifyTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "gotify-tray".into()
    }

    fn title(&self) -> String {
        "Gotify".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [22, 44].into_iter().map(|s| render(self.view.icon, self.view.unread, s)).collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "Gotify".into(), description: self.view.status_line.clone(), ..Default::default() }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        menu_model(&self.view, Local::now()).into_iter().map(to_ksni).collect()
    }

    fn menu_about_to_show(&mut self) {
        let _ = self.tx.send(TrayCommand::MenuOpened);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};
    use gotify_core::kuma_status::KumaState;

    fn view(down: Vec<DownEntry>) -> TrayView {
        TrayView {
            icon: IconKind::Ok,
            unread: false,
            status_line: "Connected to gotify.example.com".into(),
            down,
            muted: vec![],
            all: vec![],
        }
    }

    #[test]
    fn underscores_survive_the_menu() {
        // AppIndicator treats "_x" as a keyboard mnemonic and drops the "_".
        assert_eq!(menu_label("db_primary — down since 14:32"), "db__primary — down since 14:32");
        assert_eq!(menu_label("plain"), "plain");
    }

    fn web() -> MuteTarget {
        MuteTarget::Monitor("web01".into())
    }

    #[test]
    fn muted_section_shows_state_and_end() {
        let now = Local.with_ymd_and_hms(2026, 11, 5, 18, 0, 0).unwrap();
        let mut v = view(vec![]);
        v.muted = vec![
            MutedEntry {
                target: web(),
                name: "web01".into(),
                down: Some((Some(42), Local.with_ymd_and_hms(2026, 11, 5, 14, 32, 0).unwrap().to_utc())),
                until: Some(Local.with_ymd_and_hms(2026, 11, 6, 18, 0, 0).unwrap().to_utc()),
            },
            MutedEntry { target: MuteTarget::App(3), name: "CI".into(), down: None, until: None },
        ];
        let menu = menu_model(&v, now);
        let at = menu.iter().position(|e| *e == MenuEntry::Label("Muted".into())).unwrap();
        assert_eq!(
            menu[at + 1],
            MenuEntry::Sub(
                "web01 — down since 14:32, muted until Fri 18:00".into(),
                vec![
                    MenuEntry::Item("Show message".into(), TrayCommand::ShowWindow(Some(42))),
                    MenuEntry::Item("Unmute".into(), TrayCommand::Unmute(web())),
                ]
            )
        );
        assert_eq!(
            menu[at + 2],
            MenuEntry::Sub("CI — muted".into(), vec![MenuEntry::Item("Unmute".into(), TrayCommand::Unmute(MuteTarget::App(3)))])
        );
    }

    #[test]
    fn kuma_only_outage_has_no_message_and_no_clear() {
        let now = Local.with_ymd_and_hms(2026, 11, 5, 18, 0, 0).unwrap();
        let since = Local.with_ymd_and_hms(2026, 11, 5, 14, 32, 0).unwrap().to_utc();
        let menu = menu_model(&view(vec![DownEntry { monitor: "web01".into(), message_id: None, since, clearable: false }]), now);
        let MenuEntry::Sub(_, items) = &menu[3] else { panic!("{menu:?}") };
        assert!(!items.iter().any(|i| matches!(i, MenuEntry::Item(l, _) if l == "Show message" || l == "Clear")));
        assert!(items.iter().any(|i| matches!(i, MenuEntry::Item(l, _) if l == "Mute for 1 day")));
    }

    #[test]
    fn all_monitors_submenu() {
        let now = Local.with_ymd_and_hms(2026, 11, 5, 18, 0, 0).unwrap();
        let since = Local.with_ymd_and_hms(2026, 11, 5, 14, 32, 0).unwrap().to_utc();
        let mut v = view(vec![]);
        v.all = vec![
            MonitorLine { name: "api".into(), state: KumaState::Up, since: None, uptime_24h: Some(0.99987) },
            MonitorLine { name: "web".into(), state: KumaState::Down, since: Some(since), uptime_24h: Some(0.972) },
            MonitorLine { name: "backup".into(), state: KumaState::Maintenance, since: None, uptime_24h: None },
            MonitorLine { name: "new".into(), state: KumaState::Pending, since: None, uptime_24h: None },
        ];
        let menu = menu_model(&v, now);
        let open = || TrayCommand::OpenStatusPage;
        assert!(menu.contains(&MenuEntry::Sub(
            "All monitors (4)".into(),
            vec![
                MenuEntry::Item("Open status page".into(), open()),
                MenuEntry::Separator,
                MenuEntry::Item("✅ api — up · 99.98 %".into(), open()),
                MenuEntry::Item("🔴 web — down since 14:32 · 97.2 %".into(), open()),
                MenuEntry::Item("🔧 backup — maintenance".into(), open()),
                MenuEntry::Item("⏳ new — pending".into(), open()),
            ]
        )));
        assert!(!menu_model(&view(vec![]), now).iter().any(|e| matches!(e, MenuEntry::Sub(l, _) if l.starts_with("All monitors"))));
    }

    #[test]
    fn menu_without_down() {
        let now = Local::now();
        let menu = menu_model(&view(vec![]), now);
        assert_eq!(menu[0], MenuEntry::Label("Connected to gotify.example.com".into()));
        assert!(!menu.contains(&MenuEntry::Label("Down right now".into())));
        assert!(menu.contains(&MenuEntry::Item("Show messages".into(), TrayCommand::ShowWindow(None))));
        assert!(menu.contains(&MenuEntry::Item("Settings".into(), TrayCommand::Settings)));
        assert_eq!(menu.last(), Some(&MenuEntry::Item("Quit".into(), TrayCommand::Quit)));
    }

    #[test]
    fn menu_with_down() {
        let now = Local.with_ymd_and_hms(2026, 11, 5, 18, 0, 0).unwrap();
        let since = Local.with_ymd_and_hms(2026, 11, 5, 14, 32, 0).unwrap().to_utc();
        let menu = menu_model(&view(vec![DownEntry { monitor: "web01".into(), message_id: Some(42), since, clearable: true }]), now);
        let at = menu.iter().position(|e| *e == MenuEntry::Label("Down right now".into())).unwrap();
        assert_eq!(
            menu[at + 1],
            MenuEntry::Sub(
                "web01 — down since 14:32".into(),
                vec![
                    MenuEntry::Item("Show message".into(), TrayCommand::ShowWindow(Some(42))),
                    MenuEntry::Separator,
                    MenuEntry::Item("Mute for 1 day".into(), TrayCommand::Mute(web(), MuteFor::Day)),
                    MenuEntry::Item("Mute for 1 week".into(), TrayCommand::Mute(web(), MuteFor::Week)),
                    MenuEntry::Item("Mute until unmuted".into(), TrayCommand::Mute(web(), MuteFor::Forever)),
                    MenuEntry::Separator,
                    MenuEntry::Item("Clear".into(), TrayCommand::ClearMonitor("web01".into())),
                ]
            )
        );
    }
}
