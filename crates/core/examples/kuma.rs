//! Read-only check of an Uptime Kuma status page.
//! cargo run -p gotify-core --example kuma -- http://kuma:3001/status/<slug>

use gotify_core::kuma_status::{StatusPage, fetch_snapshot};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let page = StatusPage::parse(&std::env::args().nth(1).expect("status page URL")).unwrap();
    let snapshot = fetch_snapshot(&page).await.unwrap();
    println!("{:?}", snapshot.counts());
    for m in &snapshot.monitors {
        println!("{:<45} {:?} since {:?}", m.name, m.state, m.since);
    }
}
