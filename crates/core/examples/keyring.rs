//! Manual check: store, load and delete a dummy token in the GNOME keyring.
//! Run: cargo run -p gotify-core --example keyring [store|load|delete]

use gotify_core::secret::{delete_token, load_token, store_token};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let server: url::Url = "https://example.invalid/".parse().unwrap();
    match std::env::args().nth(1).as_deref() {
        Some("store") => println!("{:?}", store_token(&server, "dummy-token").await),
        Some("delete") => println!("{:?}", delete_token(&server).await),
        _ => println!("{:?}", load_token(&server).await),
    }
}
