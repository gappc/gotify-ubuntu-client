//! Tray icon (StatusNotifierItem via ksni) and its menu.

use chrono::{DateTime, Local, Utc};
use gotify_core::text::since_label;
use ksni::menu::{StandardItem, SubMenu};
use tokio::sync::mpsc::UnboundedSender;

pub use crate::icon::IconKind;
use crate::icon::render;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownEntry {
    pub monitor: String,
    pub message_id: i64,
    pub since: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    pub icon: IconKind,
    pub unread: bool,
    pub status_line: String,
    pub down: Vec<DownEntry>,
}

impl Default for TrayView {
    fn default() -> Self {
        TrayView { icon: IconKind::Offline, unread: false, status_line: "Starting…".into(), down: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow(Option<i64>),
    Settings,
    ClearMonitor(String),
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

pub fn menu_model(view: &TrayView, now: DateTime<Local>) -> Vec<MenuEntry> {
    let mut menu = vec![MenuEntry::Label(view.status_line.clone()), MenuEntry::Separator];
    if !view.down.is_empty() {
        menu.push(MenuEntry::Label("Down right now".into()));
        for d in &view.down {
            menu.push(MenuEntry::Sub(
                format!("{} — down since {}", d.monitor, since_label(d.since.with_timezone(&Local), now)),
                vec![
                    MenuEntry::Item("Show message".into(), TrayCommand::ShowWindow(Some(d.message_id))),
                    MenuEntry::Item("Clear".into(), TrayCommand::ClearMonitor(d.monitor.clone())),
                ],
            ));
        }
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

fn to_ksni(entry: MenuEntry) -> ksni::MenuItem<GotifyTray> {
    match entry {
        MenuEntry::Separator => ksni::MenuItem::Separator,
        MenuEntry::Label(label) => StandardItem { label, enabled: false, ..Default::default() }.into(),
        MenuEntry::Item(label, cmd) => StandardItem {
            label,
            activate: Box::new(move |t: &mut GotifyTray| {
                let _ = t.tx.send(cmd.clone());
            }),
            ..Default::default()
        }
        .into(),
        MenuEntry::Sub(label, items) => {
            SubMenu { label, submenu: items.into_iter().map(to_ksni).collect(), ..Default::default() }.into()
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

    fn view(down: Vec<DownEntry>) -> TrayView {
        TrayView { icon: IconKind::Ok, unread: false, status_line: "Connected to gotify.example.com".into(), down }
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
        let menu = menu_model(&view(vec![DownEntry { monitor: "web01".into(), message_id: 42, since }]), now);
        let at = menu.iter().position(|e| *e == MenuEntry::Label("Down right now".into())).unwrap();
        assert_eq!(
            menu[at + 1],
            MenuEntry::Sub(
                "web01 — down since 14:32".into(),
                vec![
                    MenuEntry::Item("Show message".into(), TrayCommand::ShowWindow(Some(42))),
                    MenuEntry::Item("Clear".into(), TrayCommand::ClearMonitor("web01".into())),
                ]
            )
        );
    }
}
