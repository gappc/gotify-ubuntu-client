//! gotify-tray-window: message list and settings. Started on demand by the
//! background service; exits when its window closes.

mod messages;
mod row;
mod rt;
mod settings;

use adw::prelude::*;
use gotify_core::ipc::WINDOW_APP_ID;

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    settings: bool,
    message: Option<i64>,
}

fn parse_args(args: &[String]) -> Args {
    let mut out = Args::default();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--settings" => out.settings = true,
            "--message" => out.message = it.next().and_then(|v| v.parse().ok()),
            _ => {}
        }
    }
    out
}

thread_local! {
    static WINDOW: std::cell::RefCell<Option<std::rc::Rc<messages::MessagesWindow>>> = const { std::cell::RefCell::new(None) };
}

fn current_window() -> Option<std::rc::Rc<messages::MessagesWindow>> {
    WINDOW.with(|w| w.borrow().clone())
}

/// Called for the first start and for every forwarded later start.
fn handle(app: &adw::Application, args: Args) {
    let window = current_window();
    if args.settings {
        let reload_target = window.clone();
        let parent = window.as_ref().map(|w| w.window.clone().upcast::<gtk::Widget>());
        let hold = app.hold();
        let dialog = settings::open_settings(parent.as_ref(), move || {
            if let Some(w) = &reload_target {
                w.reload();
            }
        });
        dialog.connect_closed(move |_| {
            let _ = &hold;
        });
        return;
    }
    let window = window.unwrap_or_else(|| {
        let w = std::rc::Rc::new(messages::build_window(app, args.message));
        WINDOW.with(|slot| *slot.borrow_mut() = Some(w.clone()));
        w.window.connect_close_request(|_| {
            WINDOW.with(|slot| slot.borrow_mut().take());
            gtk::glib::Propagation::Proceed
        });
        w
    });
    if let Some(id) = args.message {
        window.focus(id);
    }
    window.present();
}

fn main() -> gtk::glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(WINDOW_APP_ID)
        .flags(gtk::gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_startup(|app| {
        app.set_accels_for_action("window.close", &["<Control>w"]);
        let css = gtk::CssProvider::new();
        css.load_from_string("row.highlight { background-color: alpha(@accent_bg_color, 0.3); transition: background-color 400ms; }");
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    });
    app.connect_command_line(|app, cmd| {
        let args: Vec<String> = cmd.arguments().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        handle(app, parse_args(&args));
        0.into()
    });
    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_cases() {
        let a = |v: &[&str]| parse_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["w"]), Args::default());
        assert_eq!(a(&["w", "--settings"]), Args { settings: true, message: None });
        assert_eq!(a(&["w", "--message", "42"]), Args { settings: false, message: Some(42) });
        assert_eq!(a(&["w", "--message", "x"]), Args::default());
    }
}
