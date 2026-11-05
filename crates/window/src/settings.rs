//! Settings dialog: log in (create a client token) or log out.

use std::rc::Rc;

use adw::prelude::*;
use gotify_core::api::{ApiError, GotifyClient, create_client, server_info};
use gotify_core::files::{Config, Paths, load_config, remove_config, save_config};
use gotify_core::secret::{delete_token, store_token};
use url::Url;

use crate::rt::{DaemonCall, notify_daemon, spawn_ui};

pub fn normalize_server_url(input: &str) -> Result<Url, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter the address of your Gotify server.".into());
    }
    let with_scheme = if input.contains("://") { input.to_string() } else { format!("https://{input}") };
    let mut url = Url::parse(&with_scheme).map_err(|_| "That doesn't look like a web address.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("The address must start with https:// (or http://).".into());
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

pub fn client_name(hostname: &str) -> String {
    format!("Ubuntu tray ({hostname})")
}

fn explain(e: &ApiError, url: &Url) -> String {
    match e {
        ApiError::Unauthorized => "Wrong username or password.".into(),
        ApiError::Network(_) => format!("Can't reach {}. Check the address and your connection.", url.host_str().unwrap_or("the server")),
        other => other.to_string(),
    }
}

type OnChanged = Rc<dyn Fn()>;

/// Opens the settings dialog. `on_changed` runs after a login or logout.
pub fn open_settings(parent: Option<&gtk::Widget>, on_changed: impl Fn() + 'static) -> adw::PreferencesDialog {
    let dialog = adw::PreferencesDialog::builder().title("Settings").search_enabled(false).content_height(500).build();
    fill(&dialog, Rc::new(on_changed), None);
    dialog.present(parent);
    dialog
}

/// (Re)builds the dialog's page for the current login state.
fn fill(dialog: &adw::PreferencesDialog, on_changed: OnChanged, note: Option<String>) {
    if let Some(old) = dialog.visible_page() {
        dialog.remove(&old.downcast::<adw::PreferencesPage>().expect("only preference pages"));
    }
    let page = adw::PreferencesPage::new();
    let config = load_config(&Paths::from_env()).ok().flatten();
    match config {
        Some(config) => page.add(&logged_in_group(dialog, config, on_changed)),
        None => page.add(&logged_out_group(dialog, on_changed, note)),
    }
    dialog.add(&page);
}

fn status_label() -> gtk::Label {
    let label = gtk::Label::builder().wrap(true).xalign(0.0).margin_top(12).css_classes(["dim-label"]).build();
    label.set_visible(false);
    label
}

fn show(label: &gtk::Label, text: &str, error: bool) {
    label.set_text(text);
    label.set_visible(!text.is_empty());
    if error {
        label.add_css_class("error");
        label.remove_css_class("dim-label");
    } else {
        label.remove_css_class("error");
        label.add_css_class("dim-label");
    }
}

fn logged_out_group(dialog: &adw::PreferencesDialog, on_changed: OnChanged, note: Option<String>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Gotify server")
        .description("Log in once; the app creates its own client in Gotify and stores its token in the GNOME keyring. Your password is not saved.")
        .build();
    let url_row = adw::EntryRow::builder().title("Server address").build();
    let user_row = adw::EntryRow::builder().title("Username").build();
    let pass_row = adw::PasswordEntryRow::builder().title("Password").build();
    group.add(&url_row);
    group.add(&user_row);
    group.add(&pass_row);

    let test = gtk::Button::with_label("Test connection");
    let login = gtk::Button::builder().label("Log in").css_classes(["suggested-action"]).build();
    let buttons = gtk::Box::builder().spacing(12).margin_top(12).halign(gtk::Align::End).build();
    buttons.append(&test);
    buttons.append(&login);
    let status = status_label();
    let footer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    footer.append(&buttons);
    footer.append(&status);
    group.add(&footer);
    if let Some(note) = note {
        show(&status, &note, false);
    }

    {
        let (url_row, status) = (url_row.clone(), status.clone());
        test.connect_clicked(move |button| {
            let url = match normalize_server_url(&url_row.text()) {
                Ok(u) => u,
                Err(e) => return show(&status, &e, true),
            };
            button.set_sensitive(false);
            show(&status, "Checking…", false);
            let (button, status) = (button.clone(), status.clone());
            let u = url.clone();
            spawn_ui(async move { server_info(&u).await }, move |result| {
                button.set_sensitive(true);
                match result {
                    Ok(info) if !info.local_auth => {
                        show(&status, "This server only allows OIDC login, which isn't supported yet.", true)
                    }
                    Ok(info) => show(&status, &format!("Gotify {} — ready to log in", info.version), false),
                    Err(e) => show(&status, &explain(&e, &url), true),
                }
            });
        });
    }

    let dialog = dialog.clone();
    login.connect_clicked(move |button| {
        let url = match normalize_server_url(&url_row.text()) {
            Ok(u) => u,
            Err(e) => return show(&status, &e, true),
        };
        let (user, pass) = (user_row.text().to_string(), pass_row.text().to_string());
        if user.is_empty() || pass.is_empty() {
            return show(&status, "Enter your Gotify username and password.", true);
        }
        button.set_sensitive(false);
        show(&status, "Logging in…", false);
        let name = client_name(&gtk::glib::host_name());
        let u = url.clone();
        let work = async move {
            let new = create_client(&u, &user, &pass, &name).await?;
            let me = GotifyClient::new(u.clone(), new.token.clone()).current_user().await?;
            store_token(&u, &new.token).await.map_err(|e| ApiError::Decode(e.to_string()))?;
            let config = Config { server_url: u, client_id: new.id, client_name: new.name, user_name: me.name };
            save_config(&Paths::from_env(), &config).map_err(|e| ApiError::Decode(e.to_string()))?;
            Ok::<_, ApiError>(())
        };
        let (button, status, dialog, on_changed) = (button.clone(), status.clone(), dialog.clone(), on_changed.clone());
        spawn_ui(work, move |result| match result {
            Ok(()) => {
                notify_daemon(DaemonCall::ReloadConfig);
                on_changed();
                fill(&dialog, on_changed, None);
            }
            Err(e) => {
                button.set_sensitive(true);
                show(&status, &explain(&e, &url), true);
            }
        });
    });
    group
}

fn logged_in_group(dialog: &adw::PreferencesDialog, config: Config, on_changed: OnChanged) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Gotify server").build();
    let host = config.server_url.host_str().unwrap_or("Gotify").to_string();
    let row = adw::ActionRow::builder()
        .title(format!("Connected to {host}"))
        .subtitle(format!("as {}", config.user_name))
        .build();
    let logout = gtk::Button::builder().label("Log out").valign(gtk::Align::Center).css_classes(["destructive-action"]).build();
    row.add_suffix(&logout);
    group.add(&row);

    let dialog = dialog.clone();
    logout.connect_clicked(move |button| {
        button.set_sensitive(false);
        let config = config.clone();
        let work = async move {
            let token = gotify_core::secret::load_token(&config.server_url).await.ok().flatten();
            let revoked = match token {
                Some(t) => GotifyClient::new(config.server_url.clone(), t).delete_client(config.client_id).await,
                None => Err(ApiError::Forbidden),
            };
            let _ = delete_token(&config.server_url).await;
            let _ = remove_config(&Paths::from_env());
            match revoked {
                Ok(()) => None,
                Err(_) => Some(format!(
                    "Logged out. Remove the client “{}” in Gotify's web UI (Clients) to revoke it fully.",
                    config.client_name
                )),
            }
        };
        let (dialog, on_changed) = (dialog.clone(), on_changed.clone());
        spawn_ui(work, move |note| {
            notify_daemon(DaemonCall::ReloadConfig);
            on_changed();
            fill(&dialog, on_changed, note);
        });
    });
    group
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_server_url_cases() {
        assert_eq!(normalize_server_url("gotify.example.com").unwrap().as_str(), "https://gotify.example.com/");
        assert_eq!(normalize_server_url(" https://h/gotify/ ").unwrap().as_str(), "https://h/gotify/");
        assert_eq!(normalize_server_url("http://10.0.0.2:8080").unwrap().as_str(), "http://10.0.0.2:8080/");
        assert!(normalize_server_url("ftp://x").is_err());
        assert!(normalize_server_url("   ").is_err());
    }

    #[test]
    fn client_name_format() {
        assert_eq!(client_name("laptop"), "Ubuntu tray (laptop)");
    }
}
