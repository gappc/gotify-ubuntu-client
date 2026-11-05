//! gotify-tray-window: message list and settings. Started on demand by the
//! background service; exits when its window closes.

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

fn main() -> gtk::glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(WINDOW_APP_ID)
        .flags(gtk::gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_command_line(|app, cmd| {
        let args: Vec<String> = cmd.arguments().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let args = parse_args(&args);
        let window = app.active_window();
        if args.settings {
            let hold = app.hold();
            let dialog = settings::open_settings(window.as_ref().map(|w| w.upcast_ref()), || {});
            dialog.connect_closed(move |_| {
                let _ = &hold;
            });
        }
        let _ = args.message;
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
