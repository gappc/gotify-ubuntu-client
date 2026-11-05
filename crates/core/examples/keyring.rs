//! Manual check of the keyring module.
//! cargo run -p gotify-core --example keyring -- [store|load|delete] [server-url] [token]

use gotify_core::secret::{delete_token, load_token, store_token};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let server: url::Url = args.get(2).map_or("https://example.invalid/", String::as_str).parse().unwrap();
    let token = args.get(3).map_or("dummy-token", String::as_str);
    match args.get(1).map(String::as_str) {
        Some("store") => println!("{:?}", store_token(&server, token).await),
        Some("delete") => println!("{:?}", delete_token(&server).await),
        _ => println!("{:?}", load_token(&server).await),
    }
}
