//! The message window: optional app sidebar, message list, live updates.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gotify_core::api::{ApiError, GotifyClient};
use gotify_core::consts::STREAM_SILENCE_TIMEOUT;
use gotify_core::files::{Paths, load_config};
use gotify_core::model::{Application, Message, PagedMessages};
use gotify_core::secret::load_token;
use gotify_core::stream::connect_stream;

use crate::filter::{Search, matches};
use crate::row::{message_row, row_message_id};
use crate::rt::{DaemonCall, daemon_last_read, notify_daemon, runtime, spawn_ui};
use crate::settings::open_settings;

/// Why there is nothing to show.
enum Problem {
    NotConfigured,
    LoginExpired,
    Unreachable,
    Other(String),
}

impl From<ApiError> for Problem {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Unauthorized => Problem::LoginExpired,
            ApiError::Network(_) => Problem::Unreachable,
            other => Problem::Other(other.to_string()),
        }
    }
}

struct Loaded {
    client: GotifyClient,
    apps: Vec<Application>,
    first: PagedMessages,
    last_read: Option<i64>,
}

#[derive(Default)]
struct Model {
    client: Option<GotifyClient>,
    apps: Vec<Application>,
    /// None = all messages.
    filter: Option<i64>,
    /// Cursor for the next (older) page; None when history is exhausted.
    next_since: Option<i64>,
    loading: bool,
    focus: Option<i64>,
    last_read: Option<i64>,
    unread: HashMap<i64, usize>,
    stream: Option<tokio::task::JoinHandle<()>>,
    /// Bumped whenever the list is replaced; results of older requests are dropped.
    view: u64,
    search: Search,
    /// Full history of the current app filter, loaded when searching.
    history: Option<Vec<Message>>,
}

impl Model {
    fn begin_view(&mut self) -> u64 {
        self.view += 1;
        self.loading = false;
        self.view
    }

    fn is_current(&self, view: u64) -> bool {
        self.view == view
    }
}

pub struct MessagesWindow {
    pub window: adw::ApplicationWindow,
    model: Rc<RefCell<Model>>,
    ui: Rc<Ui>,
}

struct Ui {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    split: adw::OverlaySplitView,
    sidebar: gtk::ListBox,
    list: gtk::ListBox,
    scrolled: gtk::ScrolledWindow,
    stack: gtk::Stack,
    status: adw::StatusPage,
    status_button: gtk::Button,
    delete_all: gtk::Button,
    title: adw::WindowTitle,
    search_bar: gtk::SearchBar,
}

impl MessagesWindow {
    pub fn present(&self) {
        self.window.present();
    }

    /// Scroll to a message, loading older pages until it's found.
    pub fn focus(&self, id: i64) {
        if self.model.borrow().search.is_active() {
            // Leaving search reloads the full list, which then finds the message.
            self.model.borrow_mut().focus = Some(id);
            self.ui.search_bar.set_search_mode(false);
            return;
        }
        self.model.borrow_mut().focus = Some(id);
        try_focus(&self.ui, &self.model);
    }

    pub fn reload(&self) {
        reload(&self.ui, &self.model);
    }
}

pub fn build_window(app: &adw::Application, focus_message: Option<i64>) -> MessagesWindow {
    let window = adw::ApplicationWindow::builder().application(app).title("Gotify").default_width(760).default_height(680).build();

    let title = adw::WindowTitle::new("Messages", "");
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    let delete_all = gtk::Button::builder().icon_name("edit-clear-all-symbolic").tooltip_text("Delete all").build();
    header.pack_start(&delete_all);
    let menu = gtk::gio::Menu::new();
    menu.append(Some("Settings"), Some("win.settings"));
    header.pack_end(&gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).build());
    let search_button = gtk::ToggleButton::builder().icon_name("system-search-symbolic").tooltip_text("Search (Ctrl+F)").build();
    header.pack_end(&search_button);

    let search_entry = gtk::SearchEntry::builder().placeholder_text("Search messages").hexpand(true).build();
    let down_only = gtk::ToggleButton::builder()
        .label("Down only")
        .tooltip_text("Only messages shown in red: Uptime Kuma Down and other high-priority messages")
        .build();
    let search_box = gtk::Box::builder().spacing(8).width_request(420).build();
    search_box.append(&search_entry);
    search_box.append(&down_only);
    let search_bar = gtk::SearchBar::builder().child(&search_box).show_close_button(true).build();
    search_bar.connect_entry(&search_entry);
    search_bar.set_key_capture_widget(Some(&window));
    search_button.bind_property("active", &search_bar, "search-mode-enabled").bidirectional().build();

    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).valign(gtk::Align::Start).build();
    let clamp = adw::Clamp::builder().maximum_size(820).margin_top(12).margin_bottom(12).margin_start(12).margin_end(12).child(&list).build();
    let scrolled = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&clamp).build();

    let status_button = gtk::Button::builder().halign(gtk::Align::Center).css_classes(["pill", "suggested-action"]).build();
    let status = adw::StatusPage::builder().icon_name("network-offline-symbolic").child(&status_button).build();
    let spinner = adw::Spinner::new();
    let stack = gtk::Stack::new();
    stack.add_named(&scrolled, Some("list"));
    stack.add_named(&status, Some("status"));
    stack.add_named(&spinner, Some("loading"));
    stack.set_visible_child_name("loading");

    let content = adw::ToolbarView::new();
    content.add_top_bar(&header);
    content.add_top_bar(&search_bar);
    content.set_content(Some(&stack));

    let sidebar = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    sidebar_view.set_content(Some(&gtk::ScrolledWindow::builder().child(&sidebar).build()));

    let split = adw::OverlaySplitView::builder().sidebar(&sidebar_view).content(&content).show_sidebar(false).build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&split));
    window.set_content(Some(&toasts));

    let ui = Rc::new(Ui {
        window: window.clone(),
        toasts,
        split,
        sidebar,
        list,
        scrolled,
        stack,
        status,
        status_button,
        delete_all,
        title,
        search_bar: search_bar.clone(),
    });
    let model = Rc::new(RefCell::new(Model { focus: focus_message, ..Model::default() }));

    let settings = gtk::gio::SimpleAction::new("settings", None);
    {
        let (ui, model) = (ui.clone(), model.clone());
        settings.connect_activate(move |_, _| open_settings_for(&ui, &model));
    }
    window.add_action(&settings);
    let search_action = gtk::gio::SimpleAction::new("search", None);
    {
        let search_bar = search_bar.clone();
        search_action.connect_activate(move |_, _| search_bar.set_search_mode(!search_bar.is_search_mode()));
    }
    window.add_action(&search_action);
    {
        let (ui2, model) = (ui.clone(), model.clone());
        search_entry.connect_search_changed(move |entry| {
            model.borrow_mut().search.text = entry.text().to_string();
            load_list(&ui2, &model);
        });
    }
    {
        let (ui2, model) = (ui.clone(), model.clone());
        down_only.connect_toggled(move |button| {
            model.borrow_mut().search.down_only = button.is_active();
            load_list(&ui2, &model);
        });
    }
    {
        // Closing the search bar clears the search and shows everything again.
        let (ui2, model, entry, toggle) = (ui.clone(), model.clone(), search_entry.clone(), down_only.clone());
        search_bar.connect_search_mode_enabled_notify(move |bar| {
            if !bar.is_search_mode() && model.borrow().search.is_active() {
                model.borrow_mut().search = Search::default();
                entry.set_text("");
                toggle.set_active(false);
                load_list(&ui2, &model);
            }
        });
    }
    {
        let (ui2, model) = (ui.clone(), model.clone());
        ui.scrolled.connect_edge_reached(move |_, edge| {
            if edge == gtk::PositionType::Bottom {
                load_more(&ui2, &model);
            }
        });
    }
    {
        let (ui2, model) = (ui.clone(), model.clone());
        ui.sidebar.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let filter = row.widget_name().strip_prefix("app-").and_then(|id| id.parse().ok());
            if model.borrow().filter != filter {
                model.borrow_mut().filter = filter;
                model.borrow_mut().history = None;
                load_list(&ui2, &model);
            }
        });
    }
    {
        let (ui2, model) = (ui.clone(), model.clone());
        ui.delete_all.connect_clicked(move |_| confirm_delete_all(&ui2, &model));
    }
    {
        let model = model.clone();
        window.connect_close_request(move |_| {
            if let Some(task) = model.borrow_mut().stream.take() {
                task.abort();
            }
            gtk::glib::Propagation::Proceed
        });
    }
    {
        let (ui2, model) = (ui.clone(), model.clone());
        window.connect_is_active_notify(move |w| {
            if w.is_active() {
                mark_newest_read(&ui2, &model);
            }
        });
    }
    reload(&ui, &model);
    MessagesWindow { window, model, ui }
}

fn open_settings_for(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    let (ui2, model) = (ui.clone(), model.clone());
    open_settings(Some(ui.window.upcast_ref()), move || reload(&ui2, &model));
}

fn show_problem(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>, problem: Problem) {
    let host = load_config(&Paths::from_env()).ok().flatten().and_then(|c| c.server_url.host_str().map(str::to_string));
    let (icon, title, text, button) = match problem {
        Problem::NotConfigured => ("dialog-password-symbolic", "Not connected".to_string(), "Log in to your Gotify server to see messages.".to_string(), "Open Settings"),
        Problem::LoginExpired => ("dialog-password-symbolic", "Login expired".to_string(), "Log in again to see messages.".to_string(), "Open Settings"),
        Problem::Unreachable => (
            "network-offline-symbolic",
            format!("Can't reach {}", host.as_deref().unwrap_or("the server")),
            "Check your network connection.".to_string(),
            "Try again",
        ),
        Problem::Other(why) => ("dialog-warning-symbolic", "Something went wrong".to_string(), why, "Try again"),
    };
    ui.status.set_icon_name(Some(icon));
    ui.status.set_title(&title);
    ui.status.set_description(Some(&text));
    ui.status_button.set_label(button);
    let settings = button == "Open Settings";
    // Replace the previous handler by rebuilding the button's signal each time.
    let new_button = gtk::Button::builder().label(button).halign(gtk::Align::Center).css_classes(["pill", "suggested-action"]).build();
    {
        let (ui2, model) = (ui.clone(), model.clone());
        new_button.connect_clicked(move |_| {
            if settings {
                open_settings_for(&ui2, &model);
            } else {
                reload(&ui2, &model);
            }
        });
    }
    ui.status.set_child(Some(&new_button));
    ui.stack.set_visible_child_name("status");
    ui.delete_all.set_sensitive(false);
}

/// Reads config + token, then loads apps and the first page.
fn reload(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    if let Some(task) = model.borrow_mut().stream.take() {
        task.abort();
    }
    ui.stack.set_visible_child_name("loading");
    let view = model.borrow_mut().begin_view();
    let work = async {
        let Some(config) = load_config(&Paths::from_env()).ok().flatten() else { return Err(Problem::NotConfigured) };
        let token = match load_token(&config.server_url).await {
            Ok(Some(t)) => t,
            Ok(None) => return Err(Problem::NotConfigured),
            Err(e) => return Err(Problem::Other(e.to_string())),
        };
        let client = GotifyClient::new(config.server_url, token);
        let apps = client.applications().await?;
        let first = client.messages_page(None, None).await?;
        let last_read = daemon_last_read().await;
        Ok(Loaded { client, apps, first, last_read })
    };
    let (ui, model) = (ui.clone(), model.clone());
    spawn_ui(work, move |result| match result {
        _ if !model.borrow().is_current(view) => {}
        Err(problem) => show_problem(&ui, &model, problem),
        Ok(loaded) => {
            {
                let mut m = model.borrow_mut();
                m.last_read = loaded.last_read;
                m.unread = HashMap::new();
                for msg in &loaded.first.messages {
                    if m.last_read.is_some_and(|r| msg.id > r) {
                        *m.unread.entry(msg.appid).or_default() += 1;
                    }
                }
                m.apps = loaded.apps;
                m.client = Some(loaded.client);
                m.filter = None;
                m.history = None;
            }
            fill_sidebar(&ui, &model);
            if model.borrow().search.is_active() {
                load_list(&ui, &model);
            } else {
                show_page(&ui, &model, loaded.first, true);
            }
            start_stream(&ui, &model);
            mark_newest_read(&ui, &model);
        }
    });
}

fn fill_sidebar(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    while let Some(row) = ui.sidebar.row_at_index(0) {
        ui.sidebar.remove(&row);
    }
    let m = model.borrow();
    ui.split.set_show_sidebar(show_sidebar(m.apps.len()));
    let total: usize = m.unread.values().sum();
    ui.sidebar.append(&sidebar_row("All messages", None, total, None));
    for app in &m.apps {
        let row = sidebar_row(&app.name, Some(app.id), m.unread.get(&app.id).copied().unwrap_or(0), Some(app));
        ui.sidebar.append(&row);
    }
    drop(m);
    ui.sidebar.select_row(ui.sidebar.row_at_index(0).as_ref());
    // Load custom app icons.
    let m = model.borrow();
    let Some(client) = m.client.clone() else { return };
    for (index, app) in m.apps.iter().enumerate().filter(|(_, a)| a.has_custom_image()) {
        let (client, path, sidebar) = (client.clone(), app.image.clone(), ui.sidebar.clone());
        spawn_ui(async move { client.image(&path).await }, move |bytes| {
            let Ok(bytes) = bytes else { return };
            let Ok(texture) = gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(bytes)) else { return };
            let image = sidebar
                .row_at_index(index as i32 + 1)
                .and_then(|r| r.child())
                .and_then(|c| c.first_child())
                .and_downcast::<gtk::Image>();
            if let Some(image) = image {
                image.set_paintable(Some(&texture));
            }
        });
    }
}

fn sidebar_row(name: &str, app_id: Option<i64>, unread: usize, app: Option<&Application>) -> gtk::ListBoxRow {
    let row_box = gtk::Box::builder().spacing(10).build();
    let icon = gtk::Image::builder().pixel_size(24).build();
    icon.set_icon_name(Some(if app.is_some() { "application-x-executable-symbolic" } else { "mail-inbox-symbolic" }));
    row_box.append(&icon);
    row_box.append(&gtk::Label::builder().label(name).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).build());
    if unread > 0 {
        row_box.append(&gtk::Label::builder().label(unread.to_string()).css_classes(["dim-label", "numeric"]).build());
    }
    let row = gtk::ListBoxRow::builder().child(&row_box).build();
    row.set_widget_name(&app_id.map_or_else(|| "all".to_string(), |id| format!("app-{id}")));
    row
}

fn app_name(model: &Model, appid: i64) -> Option<String> {
    model.apps.iter().find(|a| a.id == appid).map(|a| a.name.clone())
}

fn add_row(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>, msg: &Message, at_top: bool) {
    let m = model.borrow();
    let name = app_name(&m, msg.appid);
    let several = m.apps.len() > 1;
    drop(m);
    let (ui2, model2) = (ui.clone(), model.clone());
    let row = message_row(msg, name.as_deref(), several, move |id| delete_one(&ui2, &model2, id));
    if at_top {
        ui.list.prepend(&row);
    } else {
        ui.list.append(&row);
    }
}

/// Puts a page into the list (replacing it when `fresh`).
fn show_page(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>, page: PagedMessages, fresh: bool) {
    if fresh {
        while let Some(row) = ui.list.row_at_index(0) {
            ui.list.remove(&row);
        }
    }
    let exhausted = page.paging.next.as_deref().is_none_or(str::is_empty) || page.messages.is_empty();
    model.borrow_mut().next_since = (!exhausted).then_some(page.paging.since);
    for msg in &page.messages {
        add_row(ui, model, msg, false);
    }
    update_title(ui, model);
    if ui.list.row_at_index(0).is_none() {
        ui.status.set_icon_name(Some("mail-read-symbolic"));
        ui.status.set_title("No messages");
        ui.status.set_description(None);
        ui.status.set_child(gtk::Widget::NONE);
        ui.stack.set_visible_child_name("status");
        ui.delete_all.set_sensitive(false);
    } else {
        ui.stack.set_visible_child_name("list");
        ui.delete_all.set_sensitive(true);
    }
    try_focus(ui, model);
}

fn update_title(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    let m = model.borrow();
    let title = m.filter.and_then(|id| app_name(&m, id)).unwrap_or_else(|| "Messages".into());
    ui.title.set_title(&title);
    let host = m.client.as_ref().and_then(|c| c.base().host_str().map(str::to_string)).unwrap_or_default();
    ui.title.set_subtitle(&host);
}

/// Loads the first page for the current filter, or the search results.
fn load_list(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    if model.borrow().search.is_active() {
        return search_list(ui, model);
    }
    let (client, filter) = {
        let m = model.borrow();
        (m.client.clone(), m.filter)
    };
    let Some(client) = client else { return };
    ui.stack.set_visible_child_name("loading");
    let view = model.borrow_mut().begin_view();
    let (ui, model) = (ui.clone(), model.clone());
    spawn_ui(async move { client.messages_page(filter, None).await }, move |result| match result {
        _ if !model.borrow().is_current(view) => {}
        Ok(page) => show_page(&ui, &model, page, true),
        Err(e) => show_problem(&ui, &model, e.into()),
    });
}

/// Search runs over the full history of the current app, loaded once.
fn search_list(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    if model.borrow().history.is_some() {
        return show_search_results(ui, model);
    }
    let (client, filter) = {
        let m = model.borrow();
        (m.client.clone(), m.filter)
    };
    let Some(client) = client else { return };
    ui.stack.set_visible_child_name("loading");
    let view = model.borrow_mut().begin_view();
    let (ui, model) = (ui.clone(), model.clone());
    spawn_ui(async move { client.all_messages_of(filter).await }, move |result| match result {
        _ if !model.borrow().is_current(view) => {}
        Ok(all) => {
            model.borrow_mut().history = Some(all);
            show_search_results(&ui, &model);
        }
        Err(e) => show_problem(&ui, &model, e.into()),
    });
}

fn show_search_results(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    model.borrow_mut().begin_view();
    while let Some(row) = ui.list.row_at_index(0) {
        ui.list.remove(&row);
    }
    let found: Vec<Message> = {
        let m = model.borrow();
        m.history
            .iter()
            .flatten()
            .filter(|msg| matches(msg, app_name(&m, msg.appid).as_deref(), &m.search))
            .cloned()
            .collect()
    };
    model.borrow_mut().next_since = None;
    for msg in &found {
        add_row(ui, model, msg, false);
    }
    update_title(ui, model);
    if found.is_empty() {
        ui.status.set_icon_name(Some("system-search-symbolic"));
        ui.status.set_title("No matching messages");
        ui.status.set_description(Some("Try other words, or turn off “Down only”."));
        ui.status.set_child(gtk::Widget::NONE);
        ui.stack.set_visible_child_name("status");
    } else {
        ui.stack.set_visible_child_name("list");
        ui.scrolled.vadjustment().set_value(0.0);
    }
    ui.delete_all.set_sensitive(false);
}

fn load_more(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    let (client, filter, since, view) = {
        let mut m = model.borrow_mut();
        if m.loading {
            return;
        }
        let (Some(client), Some(since)) = (m.client.clone(), m.next_since) else { return };
        m.loading = true;
        (client, m.filter, since, m.view)
    };
    let (ui, model) = (ui.clone(), model.clone());
    spawn_ui(async move { client.messages_page(filter, Some(since)).await }, move |result| {
        if !model.borrow().is_current(view) {
            return;
        }
        model.borrow_mut().loading = false;
        match result {
            Ok(page) => show_page(&ui, &model, page, false),
            Err(e) => toast(&ui, &format!("Couldn't load older messages: {e}")),
        }
    });
}

fn find_row(list: &gtk::ListBox, id: i64) -> Option<gtk::ListBoxRow> {
    let mut i = 0;
    while let Some(row) = list.row_at_index(i) {
        if row_message_id(&row) == Some(id) {
            return Some(row);
        }
        i += 1;
    }
    None
}

fn try_focus(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    let Some(id) = model.borrow().focus else { return };
    if let Some(row) = find_row(&ui.list, id) {
        model.borrow_mut().focus = None;
        row.add_css_class("highlight");
        let r = row.clone();
        gtk::glib::timeout_add_local_once(Duration::from_secs(3), move || r.remove_css_class("highlight"));
        // Scroll once the newly added rows have been laid out.
        let (scrolled, list) = (ui.scrolled.clone(), ui.list.clone());
        gtk::glib::timeout_add_local_once(Duration::from_millis(150), move || {
            let target = row.compute_point(&list, &gtk::graphene::Point::new(0.0, 0.0));
            let list_top = list.compute_point(&scrolled.child().expect("scrolled has a child"), &gtk::graphene::Point::new(0.0, 0.0));
            if let (Some(row_y), Some(list_y)) = (target, list_top) {
                scrolled.vadjustment().set_value(f64::from(row_y.y() + list_y.y()) - 24.0);
            }
        });
    } else if model.borrow().next_since.is_some() {
        load_more(ui, model);
    } else if ui.stack.visible_child_name().as_deref() == Some("list") {
        model.borrow_mut().focus = None;
        toast(ui, "That message no longer exists.");
    }
}

fn toast(ui: &Rc<Ui>, text: &str) {
    ui.toasts.add_toast(adw::Toast::new(text));
}

fn delete_one(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>, id: i64) {
    let Some(client) = model.borrow().client.clone() else { return };
    let (ui, model) = (ui.clone(), model.clone());
    spawn_ui(async move { client.delete_message(id).await }, move |result| match result {
        Ok(()) => {
            if let Some(history) = model.borrow_mut().history.as_mut() {
                history.retain(|m| m.id != id);
            }
            if let Some(row) = find_row(&ui.list, id) {
                // Keep the reading position: move focus to a neighbour first.
                let index = row.index();
                let neighbour = ui.list.row_at_index(index + 1).or_else(|| ui.list.row_at_index(index - 1));
                if let Some(n) = neighbour {
                    n.grab_focus();
                }
                ui.list.remove(&row);
            }
            if ui.list.row_at_index(0).is_none() {
                load_list(&ui, &model);
            }
        }
        Err(e) => toast(&ui, &format!("Couldn't delete the message: {e}")),
    });
}

fn confirm_delete_all(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    let (filter, name) = {
        let m = model.borrow();
        (m.filter, m.filter.and_then(|id| app_name(&m, id)))
    };
    let heading = match &name {
        Some(app) => format!("Delete all messages from {app}?"),
        None => "Delete all messages?".to_string(),
    };
    let dialog = adw::AlertDialog::new(Some(&heading), Some("They are deleted on the server, so they also disappear on your other devices."));
    dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    let (ui2, model2) = (ui.clone(), model.clone());
    dialog.connect_response(None, move |_, response| {
        if response != "delete" {
            return;
        }
        let Some(client) = model2.borrow().client.clone() else { return };
        let (ui, model) = (ui2.clone(), model2.clone());
        let work = async move {
            match filter {
                Some(app) => client.delete_app_messages(app).await,
                None => client.delete_all_messages().await,
            }
        };
        spawn_ui(work, move |result| match result {
            Ok(()) => {
                model.borrow_mut().history = None;
                load_list(&ui, &model);
            }
            Err(e) => toast(&ui, &format!("Couldn't delete messages: {e}")),
        });
    });
    dialog.present(Some(&ui.window));
}

/// Tells the background service everything up to the newest message is read.
fn mark_newest_read(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    if model.borrow().filter.is_some() {
        return;
    }
    if let Some(id) = ui.list.row_at_index(0).and_then(|r| row_message_id(&r)) {
        notify_daemon(DaemonCall::MarkRead(id));
    }
}

/// Live updates while the window is open; reconnects every 5 s if needed.
fn start_stream(ui: &Rc<Ui>, model: &Rc<RefCell<Model>>) {
    if let Some(old) = model.borrow_mut().stream.take() {
        old.abort();
    }
    let Some(client) = model.borrow().client.clone() else { return };
    let (tx, rx) = async_channel::unbounded::<Message>();
    let task = runtime().spawn(async move {
        loop {
            if let Ok(mut stream) = connect_stream(client.base(), client.token(), STREAM_SILENCE_TIMEOUT).await {
                while let Ok(msg) = stream.next().await {
                    if tx.send(msg).await.is_err() {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
    model.borrow_mut().stream = Some(task);
    let (ui, model) = (ui.clone(), model.clone());
    gtk::glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            let filter = model.borrow().filter;
            if filter.is_none_or(|f| f == msg.appid) {
                let (searching, shown) = {
                    let mut m = model.borrow_mut();
                    if let Some(history) = m.history.as_mut() {
                        history.insert(0, msg.clone());
                    }
                    let name = app_name(&m, msg.appid);
                    (m.search.is_active(), matches(&msg, name.as_deref(), &m.search))
                };
                if shown {
                    add_row(&ui, &model, &msg, true);
                    ui.stack.set_visible_child_name("list");
                    ui.delete_all.set_sensitive(!searching);
                }
            }
            if ui.window.is_active() && filter.is_none() {
                notify_daemon(DaemonCall::MarkRead(msg.id));
            }
        }
    });
}

/// The app sidebar is only worth showing with several apps.
pub fn show_sidebar(app_count: usize) -> bool {
    app_count > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_for_an_old_view_are_dropped() {
        let mut m = Model::default();
        let all = m.begin_view(); // e.g. "All messages" page requested
        let ci = m.begin_view(); // user switched to "CI" before it arrived
        assert!(!m.is_current(all));
        assert!(m.is_current(ci));
    }

    #[test]
    fn sidebar_hidden_for_one_app() {
        assert!(!show_sidebar(0));
        assert!(!show_sidebar(1));
        assert!(show_sidebar(2));
    }
}
