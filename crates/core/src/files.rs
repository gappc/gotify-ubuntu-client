//! config.toml (server, client id) and state.json (read/notified ids, monitors).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::monitors::Monitors;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub server_url: Url,
    pub client_id: i64,
    pub client_name: String,
    pub user_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub last_read_id: Option<i64>,
    #[serde(default)]
    pub last_notified_id: Option<i64>,
    #[serde(default)]
    pub monitors: Monitors,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_file: PathBuf,
    pub state_file: PathBuf,
}

const DIR: &str = "gotify-tray";

fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback),
    }
}

impl Paths {
    pub fn from_env() -> Paths {
        Paths {
            config_file: xdg_dir("XDG_CONFIG_HOME", ".config").join(DIR).join("config.toml"),
            state_file: xdg_dir("XDG_STATE_HOME", ".local/state").join(DIR).join("state.json"),
        }
    }

    pub fn in_dir(dir: &Path) -> Paths {
        Paths {
            config_file: dir.join("config").join(DIR).join("config.toml"),
            state_file: dir.join("state").join(DIR).join("state.json"),
        }
    }
}

/// Write to a temp file next to `path`, then rename, so a crash never
/// leaves a half-written file.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, path)
}

pub fn load_config(p: &Paths) -> io::Result<Option<Config>> {
    match std::fs::read_to_string(&p.config_file) {
        Ok(text) => toml::from_str(&text).map(Some).map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn save_config(p: &Paths, c: &Config) -> io::Result<()> {
    let text = toml::to_string(c).map_err(io::Error::other)?;
    write_atomic(&p.config_file, text.as_bytes())
}

pub fn remove_config(p: &Paths) -> io::Result<()> {
    match std::fs::remove_file(&p.config_file) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Missing or unreadable state starts fresh; a corrupt file is kept as
/// `state.json.bad` for inspection.
pub fn load_state(p: &Paths) -> State {
    let Ok(text) = std::fs::read_to_string(&p.state_file) else { return State::default() };
    match serde_json::from_str(&text) {
        Ok(state) => state,
        Err(e) => {
            eprintln!("gotify-tray: ignoring corrupt {}: {e}", p.state_file.display());
            let _ = std::fs::rename(&p.state_file, p.state_file.with_extension("json.bad"));
            State::default()
        }
    }
}

pub fn save_state(p: &Paths, s: &State) -> io::Result<()> {
    let text = serde_json::to_vec_pretty(s).map_err(io::Error::other)?;
    write_atomic(&p.state_file, &text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kuma::KumaStatus;
    use crate::monitors::MonitorRecord;
    use chrono::{TimeZone, Utc};

    fn config() -> Config {
        Config {
            server_url: "https://gotify.example.com/".parse().unwrap(),
            client_id: 11,
            client_name: "Ubuntu tray (laptop)".into(),
            user_name: "chris".into(),
        }
    }

    #[test]
    fn config_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        save_config(&p, &config()).unwrap();
        assert_eq!(load_config(&p).unwrap(), Some(config()));
        remove_config(&p).unwrap();
        assert_eq!(load_config(&p).unwrap(), None);
    }

    #[test]
    fn missing_config_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_config(&Paths::in_dir(dir.path())).unwrap(), None);
    }

    #[test]
    fn state_roundtrip_with_monitors() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        let mut s = State { last_read_id: Some(4), last_notified_id: Some(9), ..State::default() };
        s.monitors.insert(
            "web01".into(),
            MonitorRecord { status: KumaStatus::Down, message_id: 9, since: Utc.timestamp_opt(60, 0).unwrap() },
        );
        save_state(&p, &s).unwrap();
        assert_eq!(load_state(&p), s);
    }

    #[test]
    fn corrupt_state_is_default_and_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::in_dir(dir.path());
        std::fs::create_dir_all(p.state_file.parent().unwrap()).unwrap();
        std::fs::write(&p.state_file, "{not json").unwrap();
        assert_eq!(load_state(&p), State::default());
        assert!(!p.state_file.exists());
        assert!(p.state_file.with_extension("json.bad").exists());
    }
}
