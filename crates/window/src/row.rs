//! One message in the list.

use adw::prelude::*;
use gotify_core::kuma::{KumaStatus, parse_kuma};
use gotify_core::model::Message;
use gotify_core::rules::display_title;
use gotify_core::text::{Segment, escape_markup, relative_time, segments};

/// Pango markup for the message body: escaped text, links as `<a>`.
/// Kuma messages show only the details (the monitor is in the title).
pub fn row_markup(msg: &Message) -> String {
    let text = parse_kuma(&msg.message).map_or_else(|| msg.message.clone(), |e| e.details);
    segments(&text)
        .into_iter()
        .map(|seg| match seg {
            Segment::Text(t) => escape_markup(t),
            Segment::Link(url) => {
                let url = escape_markup(url);
                format!("<a href=\"{url}\">{url}</a>")
            }
        })
        .collect()
}

pub fn row_title(msg: &Message, app_name: Option<&str>) -> String {
    match parse_kuma(&msg.message) {
        Some(e) if e.status == KumaStatus::Down => format!("🔴 {}", e.monitor),
        Some(e) => format!("✅ {}", e.monitor),
        None => display_title(msg, app_name),
    }
}

fn is_alert(msg: &Message) -> bool {
    match parse_kuma(&msg.message) {
        Some(e) => e.status == KumaStatus::Down,
        None => msg.priority >= 8,
    }
}

/// Builds a list row. `show_app` adds the app name (when there are several
/// apps); `on_delete` is called with the message id.
pub fn message_row(msg: &Message, app_name: Option<&str>, show_app: bool, on_delete: impl Fn(i64) + 'static) -> gtk::ListBoxRow {
    let title = gtk::Label::builder()
        .label(row_title(msg, app_name))
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .css_classes(["heading"])
        .build();
    if is_alert(msg) {
        title.add_css_class("error");
    }
    let when = relative_time(msg.date, chrono::Utc::now());
    let meta_text = match (show_app, app_name) {
        (true, Some(app)) => format!("{app} · {when}"),
        _ => when,
    };
    let meta = gtk::Label::builder().label(meta_text).css_classes(["dim-label", "caption"]).valign(gtk::Align::Start).build();
    meta.set_tooltip_text(Some(&msg.date.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string()));
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Delete message")
        .valign(gtk::Align::Start)
        .css_classes(["flat", "circular"])
        .build();
    let id = msg.id;
    delete.connect_clicked(move |_| on_delete(id));

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.append(&title);
    top.append(&meta);
    top.append(&delete);

    let body = gtk::Label::builder()
        .use_markup(true)
        .label(row_markup(msg))
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .build();
    body.connect_activate_link(|label, uri| {
        let window = label.root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(uri).launch(window.as_ref(), gtk::gio::Cancellable::NONE, |_| {});
        gtk::glib::Propagation::Stop
    });

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(6)
        .build();
    content.append(&top);
    if !body.label().is_empty() {
        content.append(&body);
    }
    let row = gtk::ListBoxRow::builder().child(&content).activatable(false).build();
    row.set_widget_name(&format!("msg-{id}"));
    row
}

pub fn row_message_id(row: &gtk::ListBoxRow) -> Option<i64> {
    row.widget_name().strip_prefix("msg-")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn m(text: &str) -> Message {
        Message { id: 1, appid: 1, message: text.into(), title: Some("Deploy".into()), priority: 5, date: Utc::now(), extras: None }
    }

    #[test]
    fn row_markup_escapes_and_links() {
        assert_eq!(row_markup(&m("a<b https://x.y/z")), "a&lt;b <a href=\"https://x.y/z\">https://x.y/z</a>");
    }

    #[test]
    fn row_markup_kuma_uses_details() {
        assert_eq!(row_markup(&m("[web01] [🔴 Down] timeout & more")), "timeout &amp; more");
    }

    #[test]
    fn row_title_marks_kuma_status() {
        assert_eq!(row_title(&m("[web01] [🔴 Down] x"), None), "🔴 web01");
        assert_eq!(row_title(&m("[web01] [✅ Up] x"), None), "✅ web01");
        assert_eq!(row_title(&m("plain"), Some("CI")), "Deploy");
    }
}
