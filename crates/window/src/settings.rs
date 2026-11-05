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

pub fn normalize_token(input: &str) -> Result<String, String> {
    let token = input.trim();
    if token.is_empty() {
        return Err("Paste the client token from Gotify's web UI (Clients).".into());
    }
    Ok(token.to_string())
}

pub fn explain_token_error(e: &ApiError, url: &Url) -> String {
    match e {
        ApiError::Unauthorized => {
            "Gotify doesn't accept this token. Use a client token (web UI → Clients), not an app token.".into()
        }
        other => explain(other, url),
    }
}

/// What to tell the user after logging out (`revoked`: the client was deleted).
pub fn logout_note(config: &Config, revoked: bool) -> Option<String> {
    if revoked {
        return None;
    }
    Some(match &config.client_name {
        Some(name) => format!("Logged out. Remove the client “{name}” in Gotify's web UI (Clients) to revoke it fully."),
        None => "Logged out. Delete the client you created for this app in Gotify's web UI (Clients) to revoke its token."
            .into(),
    })
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
    const TOKEN_HELP: &str = "In Gotify's web UI, open Clients, create a client (e.g. “ubuntu-tray”) and paste its token here. The token is stored in the GNOME keyring.";
    const PASSWORD_HELP: &str = "The app creates its own client in Gotify and stores its token in the GNOME keyring. Your password is not saved.";
    let group = adw::PreferencesGroup::builder().title("Gotify server").description(TOKEN_HELP).build();
    let url_row = adw::EntryRow::builder().title("Server address").build();
    let token_row = adw::PasswordEntryRow::builder().title("Client token").build();
    let user_row = adw::EntryRow::builder().title("Username").visible(false).build();
    let pass_row = adw::PasswordEntryRow::builder().title("Password").visible(false).build();
    for row in [url_row.upcast_ref::<gtk::Widget>(), token_row.upcast_ref(), user_row.upcast_ref(), pass_row.upcast_ref()] {
        group.add(row);
    }

    let switch_mode = gtk::Button::builder().label("Log in with password instead").css_classes(["flat"]).build();
    let test = gtk::Button::with_label("Test connection");
    let login = gtk::Button::builder().label("Log in").css_classes(["suggested-action"]).build();
    let buttons = gtk::Box::builder().spacing(12).margin_top(12).build();
    switch_mode.set_hexpand(true);
    switch_mode.set_halign(gtk::Align::Start);
    buttons.append(&switch_mode);
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
        let (group, token_row, user_row, pass_row, status) =
            (group.clone(), token_row.clone(), user_row.clone(), pass_row.clone(), status.clone());
        switch_mode.connect_clicked(move |button| {
            let to_password = token_row.is_visible();
            token_row.set_visible(!to_password);
            user_row.set_visible(to_password);
            pass_row.set_visible(to_password);
            group.set_description(Some(if to_password { PASSWORD_HELP } else { TOKEN_HELP }));
            button.set_label(if to_password { "Use a client token instead" } else { "Log in with password instead" });
            show(&status, "", false);
        });
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
        let with_token = token_row.is_visible();
        let finish = |button: &gtk::Button, status: &gtk::Label| {
            button.set_sensitive(false);
            show(status, "Logging in…", false);
        };
        let (button2, status2, dialog2, on_changed2, url2) =
            (button.clone(), status.clone(), dialog.clone(), on_changed.clone(), url.clone());
        let done = move |result: Result<(), String>| match result {
            Ok(()) => {
                notify_daemon(DaemonCall::ReloadConfig);
                on_changed2();
                fill(&dialog2, on_changed2, None);
            }
            Err(text) => {
                button2.set_sensitive(true);
                show(&status2, &text, true);
            }
        };
        if with_token {
            let token = match normalize_token(&token_row.text()) {
                Ok(t) => t,
                Err(e) => return show(&status, &e, true),
            };
            finish(button, &status);
            let work = async move {
                let me = GotifyClient::new(url2.clone(), token.clone())
                    .current_user()
                    .await
                    .map_err(|e| explain_token_error(&e, &url2))?;
                save_login(&url2, &token, None, None, me.name).await
            };
            spawn_ui(work, done);
        } else {
            let (user, pass) = (user_row.text().to_string(), pass_row.text().to_string());
            if user.is_empty() || pass.is_empty() {
                return show(&status, "Enter your Gotify username and password.", true);
            }
            finish(button, &status);
            let name = client_name(&gtk::glib::host_name());
            let work = async move {
                let new = create_client(&url2, &user, &pass, &name).await.map_err(|e| explain(&e, &url2))?;
                let me = GotifyClient::new(url2.clone(), new.token.clone())
                    .current_user()
                    .await
                    .map_err(|e| explain(&e, &url2))?;
                save_login(&url2, &new.token, Some(new.id), Some(new.name), me.name).await
            };
            spawn_ui(work, done);
        }
    });
    group
}

/// Token into the keyring, everything else into config.toml.
async fn save_login(
    url: &Url,
    token: &str,
    client_id: Option<i64>,
    client_name: Option<String>,
    user_name: String,
) -> Result<(), String> {
    store_token(url, token).await.map_err(|e| format!("Couldn't save the token: {e}"))?;
    let config = Config { server_url: url.clone(), client_id, client_name, user_name };
    save_config(&Paths::from_env(), &config).map_err(|e| format!("Couldn't save the settings: {e}"))
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
            let revoked = match (token, config.client_id) {
                (Some(t), Some(id)) => GotifyClient::new(config.server_url.clone(), t).delete_client(id).await.is_ok(),
                _ => false,
            };
            let _ = delete_token(&config.server_url).await;
            let _ = remove_config(&Paths::from_env());
            logout_note(&config, revoked)
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
    fn token_is_trimmed() {
        assert_eq!(normalize_token("  gtfyc.abc\n").unwrap(), "gtfyc.abc");
        assert!(normalize_token("   ").is_err());
    }

    #[test]
    fn rejected_token_is_explained() {
        let url: Url = "https://g.example/".parse().unwrap();
        assert_eq!(
            explain_token_error(&ApiError::Unauthorized, &url),
            "Gotify doesn't accept this token. Use a client token (web UI → Clients), not an app token."
        );
    }

    #[test]
    fn logout_note_depends_on_how_we_logged_in() {
        let mut c = Config {
            server_url: "https://g.example/".parse().unwrap(),
            client_id: Some(3),
            client_name: Some("Ubuntu tray (laptop)".into()),
            user_name: "chris".into(),
        };
        assert_eq!(logout_note(&c, true), None);
        assert_eq!(
            logout_note(&c, false).unwrap(),
            "Logged out. Remove the client “Ubuntu tray (laptop)” in Gotify's web UI (Clients) to revoke it fully."
        );
        c.client_id = None;
        c.client_name = None;
        assert_eq!(
            logout_note(&c, false).unwrap(),
            "Logged out. Delete the client you created for this app in Gotify's web UI (Clients) to revoke its token."
        );
    }

    #[test]
    fn client_name_format() {
        assert_eq!(client_name("laptop"), "Ubuntu tray (laptop)");
    }
}
