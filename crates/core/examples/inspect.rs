//! Read-only check against a real server: parses all messages and prints
//! counts, monitor names and the current down list (no message text).
//! GOTIFY_URL=... GOTIFY_TOKEN=... cargo run -p gotify-core --example inspect

use gotify_core::api::GotifyClient;
use gotify_core::kuma::parse_kuma;
use gotify_core::monitors::{Monitors, down_list, resync};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let url = std::env::var("GOTIFY_URL").expect("GOTIFY_URL").parse().expect("valid URL");
    let client = GotifyClient::new(url, std::env::var("GOTIFY_TOKEN").expect("GOTIFY_TOKEN"));
    let apps = client.applications().await.expect("applications");
    let messages = client.all_messages().await.expect("messages");
    let kuma = messages.iter().filter(|m| parse_kuma(&m.message).is_some()).count();
    println!("apps: {}, messages: {}, Uptime Kuma format: {}", apps.len(), messages.len(), kuma);
    let monitors = resync(&Monitors::new(), &messages);
    println!("monitors: {:?}", monitors.keys().collect::<Vec<_>>());
    let down: Vec<_> = down_list(&monitors).into_iter().map(|(name, r)| format!("{name} (since {})", r.since)).collect();
    println!("down right now: {down:?}");
}
