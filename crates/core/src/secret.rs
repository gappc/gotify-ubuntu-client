//! Client token storage in the GNOME keyring (Secret Service over D-Bus).
//!
//! Never unlocks the keyring itself: a locked keyring is reported as
//! `SecretError::Locked` so the caller can wait instead of popping up a prompt.

use oo7::dbus::{Collection, Service};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("the keyring is locked")]
    Locked,
    #[error("the keyring is not available: {0}")]
    Unavailable(String),
}

const LABEL: &str = "Gotify tray token";

fn attributes(server: &Url) -> [(&'static str, String); 2] {
    [("application", "gotify-tray".to_string()), ("server", server.to_string())]
}

fn unavailable(e: oo7::dbus::Error) -> SecretError {
    SecretError::Unavailable(e.to_string())
}

async fn unlocked_collection() -> Result<Collection, SecretError> {
    let service = Service::new().await.map_err(unavailable)?;
    // `default_collection()` would create a missing keyring, which prompts.
    let collection = service
        .with_alias(Service::DEFAULT_COLLECTION)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| SecretError::Unavailable("there is no default keyring".into()))?;
    if collection.is_locked().await.map_err(unavailable)? {
        return Err(SecretError::Locked);
    }
    Ok(collection)
}

pub async fn load_token(server: &Url) -> Result<Option<String>, SecretError> {
    let collection = unlocked_collection().await?;
    let items = collection.search_items(&attributes(server)).await.map_err(unavailable)?;
    let Some(item) = items.first() else { return Ok(None) };
    let secret = item.secret().await.map_err(unavailable)?;
    Ok(secret.as_str().map(str::to_string))
}

pub async fn store_token(server: &Url, token: &str) -> Result<(), SecretError> {
    let collection = unlocked_collection().await?;
    collection
        .create_item(LABEL, &attributes(server), oo7::Secret::text(token), true, None)
        .await
        .map(drop)
        .map_err(unavailable)
}

pub async fn delete_token(server: &Url) -> Result<(), SecretError> {
    let collection = unlocked_collection().await?;
    for item in collection.search_items(&attributes(server)).await.map_err(unavailable)? {
        item.delete(None).await.map_err(unavailable)?;
    }
    Ok(())
}
