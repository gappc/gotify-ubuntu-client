//! Prints the tray menu as text, built from a real status page (read-only).
//! cargo run -p gotify-tray --example menu_preview -- http://kuma:3001/status/<slug>

use std::collections::HashMap;

use gotify_core::kuma_status::{StatusPage, fetch_snapshot};
use gotify_core::monitors::Monitors;
use gotify_core::mutes::Mutes;
use gotify_tray::app::{Conn, KumaView, tray_view_at};
use gotify_tray::tray::{MenuEntry, menu_model};

fn print(entries: &[MenuEntry], indent: usize) {
    for e in entries {
        let pad = " ".repeat(indent);
        match e {
            MenuEntry::Label(l) => println!("{pad}{l}"),
            MenuEntry::Separator => println!("{pad}────────────"),
            MenuEntry::Item(l, _) => println!("{pad}{l}"),
            MenuEntry::Sub(l, items) => {
                println!("{pad}{l}  ▸");
                print(items, indent + 4);
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let page = StatusPage::parse(&std::env::args().nth(1).expect("status page URL")).unwrap();
    let snapshot = fetch_snapshot(&page).await.unwrap();
    let now = chrono::Utc::now();
    let view = tray_view_at(&Conn::Connected, Some("gotify"), &Monitors::new(), 0, &Mutes::default(), &HashMap::new(), now, &KumaView::Ok(&snapshot));
    print(&menu_model(&view, chrono::Local::now()), 0);
}
