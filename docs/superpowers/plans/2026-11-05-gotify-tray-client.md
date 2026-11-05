# Gotify Tray Client Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A tiny Ubuntu/GNOME background app that stays connected to one Gotify 3 server, shows desktop notifications (Uptime Kuma aware), shows "down right now" in a tray icon, and opens a GTK message window on demand.

**Architecture:** Cargo workspace with three crates. `gotify-core` (library) holds all pure logic (Kuma parsing, notification rules, down-state, missed messages), the Gotify HTTP/WebSocket client, config/state files, keyring access and the D-Bus interface definition. `gotify-tray` (binary, **no GTK**) is the always-running background process: connection, tray icon (`ksni`), notifications (D-Bus via `zbus`), D-Bus service. `gotify-tray-window` (binary, GTK4 + libadwaita) is started by the background process when needed and exits when its window closes.

**Tech Stack:** Rust 1.99 (edition 2024), tokio (current-thread in the daemon), reqwest 0.13 (rustls, no OpenSSL), tokio-tungstenite 0.30 (rustls), zbus 6, ksni 0.3.7, oo7 0.7, gtk4 0.11 + libadwaita 0.9, serde/serde_json, toml, chrono, thiserror, wiremock (tests).

**Spec:** `docs/superpowers/specs/2026-11-05-gotify-tray-client-design.md` — read it before starting any task.

## Global Constraints

- Background process (`gotify-tray`) must not link GTK. **≤ 15 MB resident memory** idle; no periodic wakeups beyond the WebSocket keepalive, the 15-minute re-sync and (only while the keyring is locked) the keyring retry.
- Gotify 3.1.0 API: `limit` ≤ **200**; page with `since`; `paging.next` is relative. Client token sent as `X-Gotify-Key` header (never in the URL query, so it doesn't end up in logs).
- Login: `POST /client` with HTTP Basic auth, body `{"name": "Ubuntu tray (<hostname>)"}`, no `expiresAfterInactivitySeconds`. Password never stored. Token only in the GNOME keyring.
- Kuma message format: `[<monitor name>] [🔴 Down] <details>` / `[<monitor name>] [✅ Up] <details>`.
- Priority mapping (non-Kuma): 8–10 critical, 4–7 normal, 1–3 low, 0 no notification. Kuma: Down → critical, Up → normal, regardless of priority.
- Server is public HTTPS with a trusted certificate. Use rustls with webpki or native roots; no custom CA support.
- Never pop up a keyring unlock prompt.
- Constants (put them in `gotify-core/src/consts.rs`): `PAGE_LIMIT = 200`, `BACKOFF_START = 1 s`, `BACKOFF_MAX = 60 s`, `STREAM_SILENCE_TIMEOUT = 100 s`, `RESYNC_INTERVAL = 15 min`, `MENU_RESYNC_DEBOUNCE = 10 s`, `KEYRING_RETRY = 30 s`.
- Names: daemon binary `gotify-tray`, window binary `gotify-tray-window`, desktop file id `gotify-tray`, D-Bus name `dev.gotifytray.GotifyTray`, object path `/dev/gotifytray/GotifyTray`, interface `dev.gotifytray.GotifyTray1`, GTK application id `dev.gotifytray.GotifyTray.Window` (all placeholders the spec allows; keep them consistent).
- Files: `$XDG_CONFIG_HOME/gotify-tray/config.toml` (default `~/.config`), `$XDG_STATE_HOME/gotify-tray/state.json` (default `~/.local/state`).
- User-facing text is English.
- **Never write to the user's real Gotify server during development.** Integration tests and manual checks use a local Gotify in Docker (`gotify/server:3.1.0`, see Task 7). A read-only token for the real server exists in `~/.config/gotify-check` (`GOTIFY_URL`, `GOTIFY_TOKEN`); only GET requests are allowed with it, and only for the final check in Task 13.
- Commits: plain messages, no co-author trailer, no mention of AI/Claude.

## Review Focus

1. **Stream messages arriving while the initial/missed sync is running** → each message notified exactly once, none lost. Test in Task 10 (`sync_buffers_stream_messages`).
2. **Suspend/resume or network change leaving a half-dead WebSocket** → reconnect within `STREAM_SILENCE_TIMEOUT` or immediately on the logind/NetworkManager signal. Test in Task 6 (`stream_reports_silence`) plus manual check in Task 10.
3. **Base URL with a path prefix or trailing slash** (`https://host/gotify/`) → all endpoints and `paging.next` resolve under the prefix. Test in Task 5 (`urls_respect_path_prefix`).
4. **Monitor name containing `]` or unusual characters, and message text containing `<`, `&`** → parsed correctly, notification body escaped. Tests in Task 2 (`parse_name_with_bracket`) and Task 8 (`body_is_escaped`).
5. **Token revoked in the web UI while running (401 on stream or API)** → grey icon, "Login expired" status, exactly one notification, no reconnect loop. Test in Task 10 (`unauthorized_stops_reconnecting`).

---

## File Structure

```
Cargo.toml                         workspace
crates/core/src/lib.rs             module list
crates/core/src/consts.rs          constants above
crates/core/src/model.rs           serde models (Message, Application, PagedMessages, ...)
crates/core/src/kuma.rs            Kuma message parsing
crates/core/src/rules.rs           delivery (urgency) + display title
crates/core/src/monitors.rs        down-state per monitor
crates/core/src/inbox.rs           missed-message notice, unread count
crates/core/src/text.rs            URL detection, markup escape, time labels
crates/core/src/files.rs           config.toml + state.json
crates/core/src/api.rs             Gotify HTTP client
crates/core/src/stream.rs          Gotify WebSocket stream
crates/core/src/secret.rs          GNOME keyring (oo7)
crates/core/src/ipc.rs             D-Bus names + client proxy for the daemon interface
crates/daemon/src/main.rs          single instance, startup
crates/daemon/src/app.rs           event loop + connection state machine
crates/daemon/src/backoff.rs       reconnect backoff
crates/daemon/src/notify.rs        D-Bus notifications
crates/daemon/src/tray.rs          ksni tray + TrayView
crates/daemon/src/icon.rs          tray pixmaps
crates/daemon/src/service.rs       D-Bus service implementation
crates/daemon/src/signals.rs       logind / NetworkManager signals
crates/window/src/main.rs          GtkApplication, CLI args
crates/window/src/rt.rs            tokio runtime + spawn_ui helper
crates/window/src/settings.rs      settings/login dialog
crates/window/src/messages.rs      message window (sidebar, list, delete, live)
crates/window/src/row.rs           message row widget
data/gotify-tray.desktop           app menu + autostart entry
install.sh                         build + install / --uninstall
tests/docker-compose.yml           local Gotify for integration tests
README.md
```

---

### Task 1: Workspace and models

**Files:**
- Create: `Cargo.toml`, `crates/core/Cargo.toml`, `crates/core/src/lib.rs`, `crates/core/src/consts.rs`, `crates/core/src/model.rs`, `crates/daemon/Cargo.toml`, `crates/daemon/src/main.rs` (prints version), `crates/window/Cargo.toml`, `crates/window/src/main.rs` (prints version)
- Test: `crates/core/src/model.rs` (unit tests), fixture `crates/core/tests/fixtures/messages.json`

**Interfaces:**
- Produces (all `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]`):
  - `Message { id: i64, appid: i64, message: String, title: Option<String>, priority: i64, date: DateTime<Utc>, extras: Option<serde_json::Value> }`
  - `Application { id: i64, name: String, description: String, image: String, default_priority: i64 /* serde rename "defaultPriority" */ }` with `fn has_custom_image(&self) -> bool` (false when `image` contains `static/defaultapp.png` or is empty)
  - `Paging { size: i64, since: i64, limit: i64, next: Option<String> }`, `PagedMessages { paging: Paging, messages: Vec<Message> }`
  - `VersionInfo { version: String }`, `GotifyInfo { version: String, local_auth: bool /* "localAuth" */, oidc: bool }`, `User { id: i64, name: String }`, `NewClient { id: i64, token: String, name: String }`
- Unknown JSON fields are ignored everywhere.

- [ ] **Step 1: Create the workspace.** Root `Cargo.toml` with `members = ["crates/*"]`, `resolver = "3"`, shared `[workspace.dependencies]`, `[profile.release] lto = true, codegen-units = 1, strip = true, panic = "abort", opt-level = "s"` (small binary/memory). Crate package names: `gotify-core`, `gotify-tray`, `gotify-tray-window`. Add `/target` is already in `.gitignore`.
- [ ] **Step 2: Write failing tests** in `model.rs`:
  - `parses_paged_messages`: fixture with 2 messages (one with `extras`, one without `title`) and `paging.next = "/message?limit=2&since=5"` → `messages.len() == 2`, `paging.next == Some("/message?limit=2&since=5")`, `messages[1].title == None`, date parses with nanoseconds (`2026-11-04T15:10:07.736095042Z`).
  - `default_image_is_not_custom`: `image = "static/defaultapp.png"` → `false`; `image = "image/abc.png"` → `true`.
  - `gotify_info_parses`: `{"version":"3.1.0","register":false,"localAuth":true,"oidc":false,"oidcIdpName":"OIDC","oidcAutoRedirect":false}` → `local_auth == true`, `oidc == false`.
- [ ] **Step 3: Run** `cargo test -p gotify-core` → FAIL (types missing).
- [ ] **Step 4: Implement** the models and `consts.rs` (values from Global Constraints).
- [ ] **Step 5: Run** `cargo test -p gotify-core` → PASS; `cargo build --workspace` → OK.
- [ ] **Step 6: Commit** `git commit -m "Set up workspace and Gotify models"`

### Task 2: Kuma parsing and notification rules

**Files:**
- Create: `crates/core/src/kuma.rs`, `crates/core/src/rules.rs`

**Interfaces:**
- Consumes: `Message` (Task 1)
- Produces:
  - `enum KumaStatus { Down, Up }` (Serialize/Deserialize, Copy, Eq)
  - `struct KumaEvent { monitor: String, status: KumaStatus, details: String }`
  - `fn parse_kuma(text: &str) -> Option<KumaEvent>`
  - `enum Urgency { Low, Normal, Critical }` (Copy, Eq, Ord: Low < Normal < Critical)
  - `enum Delivery { Silent, Notify(Urgency) }`
  - `fn delivery_for(msg: &Message) -> Delivery`
  - `fn display_title(msg: &Message, app_name: Option<&str>) -> String`

- [ ] **Step 1: Write failing tests** (`kuma.rs`):
  - `parse_down`: `"[web01] [🔴 Down] timeout of 48000ms exceeded"` → monitor `web01`, Down, details `timeout of 48000ms exceeded`
  - `parse_up`: `"[web01] [✅ Up] 200 - OK"` → Up, details `200 - OK`
  - `parse_without_details`: `"[web01] [✅ Up]"` → details `""`
  - `parse_name_with_bracket`: `"[api [eu]] [🔴 Down] x"` → monitor `api [eu]`
  - `parse_rejects_other`: `"hello"`, `"[web01] [⚠ Pending] x"`, `"web01 [🔴 Down] x"` → `None`
  - Algorithm: text must start with `[`; find the **first** occurrence of `"] [🔴 Down]"` or `"] [✅ Up]"` (whichever comes first); monitor = text between the leading `[` and that match; details = remainder with one leading space stripped. Monitor must be non-empty.
- [ ] **Step 2: Write failing tests** (`rules.rs`), building `Message`s with a helper `msg(priority, text)`:
  - `kuma_down_is_critical_even_at_low_priority`: priority 2, Down text → `Notify(Critical)`
  - `kuma_up_is_normal_even_at_high_priority`: priority 8, Up text → `Notify(Normal)`
  - `priority_ranges`: 10, 8 → Critical; 7, 4 → Normal; 3, 1 → Low; 0, -1 → Silent; 11 → Critical
  - `title_uses_monitor_name`: Kuma message with title `Uptime-Kuma` → `"web01"`
  - `title_falls_back`: non-Kuma with title `"Deploy"` → `"Deploy"`; title `None` or `""` with app name `"CI"` → `"CI"`; neither → `"Gotify"`
- [ ] **Step 3: Run** `cargo test -p gotify-core kuma rules` → FAIL.
- [ ] **Step 4: Implement** both modules (no regex crate needed).
- [ ] **Step 5: Run** → PASS.
- [ ] **Step 6: Commit** `"Add Uptime Kuma parsing and notification rules"`

### Task 3: Down-state per monitor

**Files:**
- Create: `crates/core/src/monitors.rs`

**Interfaces:**
- Consumes: `Message`, `parse_kuma`, `KumaStatus`
- Produces:
  - `struct MonitorRecord { status: KumaStatus, message_id: i64, since: DateTime<Utc> }` (Serialize/Deserialize)
  - `type Monitors = BTreeMap<String, MonitorRecord>`
  - `fn apply_live(monitors: &mut Monitors, msg: &Message)` — ignores non-Kuma messages and messages with `id <= record.message_id`
  - `fn resync(known: &Monitors, server: &[Message]) -> Monitors`
  - `fn clear(monitors: &mut Monitors, name: &str)` — sets status `Up`, keeps `message_id` and `since`
  - `fn down_list(monitors: &Monitors) -> Vec<(&str, &MonitorRecord)>` — only Down, oldest `since` first

`resync` rule (from the spec: deletions only ever clear): for every monitor in `known` ∪ Kuma messages on the server, take the server's newest Kuma message for that monitor and the known record; the one with the larger message id wins. If the winner is Down and its message id is **not** among the server's message ids, the result is `Up` with the same `message_id`/`since`. `since` = the date of the winning message.

- [ ] **Step 1: Write failing tests** (helper `down(id, name)`, `up(id, name)` messages; ids double as dates for readability):
  - `first_run_takes_server_state`: known `{}`, server `[down(1,"web")]` → web Down, id 1
  - `up_after_down`: server `[up(2,"web"), down(1,"web")]` → Up
  - `deleted_down_clears`: known web Down id 3; server `[up(2), down(1)]` → Up, id 3
  - `deleted_up_does_not_revive`: known web Up id 2; server `[down(1)]` → Up, id 2
  - `still_down`: known Down id 1; server `[down(1)]` → Down
  - `newer_up_on_server_wins`: known Down id 1; server `[up(5), down(1)]` → Up id 5
  - `clear_survives_resync_until_newer_message`: known Down id 1 → `clear` → resync with `[down(1)]` → Up; then `apply_live(down(7))` → Down id 7
  - `apply_live_ignores_older`: record Up id 5; `apply_live(down(4))` → still Up
  - `non_kuma_ignored`: server message `"hello"` → no monitors
  - `down_list_sorted`: two Down monitors → oldest first
- [ ] **Step 2: Run** `cargo test -p gotify-core monitors` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `"Add monitor down-state tracking"`

### Task 4: Missed messages, unread count, text helpers

**Files:**
- Create: `crates/core/src/inbox.rs`, `crates/core/src/text.rs`

**Interfaces:**
- Consumes: `Message`, `delivery_for`, `display_title`, `Urgency`
- Produces:
  - `enum MissedNotice { None, Single(Message), Summary { count: usize, urgency: Urgency, lines: Vec<String> } }`
  - `fn missed_notice(last_notified: Option<i64>, newest_first: &[Message], app_name: impl Fn(i64) -> Option<String>) -> (MissedNotice, Option<i64>)` — second value = new `last_notified`
  - `fn unread_count(last_read: Option<i64>, ids: impl IntoIterator<Item = i64>) -> usize`
  - `fn first_url(text: &str) -> Option<&str>`
  - `enum Segment<'a> { Text(&'a str), Link(&'a str) }`, `fn segments(text: &str) -> Vec<Segment<'_>>`
  - `fn escape_markup(text: &str) -> String` (`&` `<` `>` `"` `'`)
  - `fn relative_time(t: DateTime<Utc>, now: DateTime<Utc>) -> String`
  - `fn since_label(t: DateTime<Local>, now: DateTime<Local>) -> String`

Rules for `missed_notice`: `last_notified == None` (first run) → `(None, max id)`; no notification. Otherwise consider messages with `id > last_notified` whose delivery isn't `Silent`: 0 → `None`; 1 → `Single`; more → `Summary` with `count`, `urgency` = highest urgency among them, `lines` = up to 3 newest as `"{display_title}: {details or message}"` (Kuma: details; others: message), plus `"…and {k} more"` when more than 3. New `last_notified` = max of old value and all ids seen (silent ones included). `unread_count(None, ..)` = 0.

- [ ] **Step 1: Write failing tests** (`inbox.rs`): `first_run_notifies_nothing`, `nothing_new`, `single_missed`, `summary_of_five` (count 5, 4 lines with `"…and 2 more"`), `summary_is_critical_if_any_down`, `silent_messages_not_counted_but_advance_last_notified`, `unread_counts_newer_ids`, `unread_none_is_zero`.
- [ ] **Step 2: Write failing tests** (`text.rs`):
  - `first_url_strips_trailing_punctuation`: `"see https://ci.example.com/job/1."` → `https://ci.example.com/job/1`; `"(http://a.b/c)"` → `http://a.b/c`; `"no link"` → `None`
  - `segments_split`: `"a https://x.y b"` → `[Text("a "), Link("https://x.y"), Text(" b")]`
  - `escape`: `"<b>&"` → `"&lt;b&gt;&amp;"`
  - `relative_time_labels`: 30 s → `"just now"`, 5 min → `"5 min ago"`, 3 h → `"3 h ago"`, 1 day (calendar yesterday) → `"yesterday"`, older → `"12 Oct"`, older and previous year → `"12 Oct 2025"`
  - `since_label_formats`: same day → `"14:32"`; within 6 days → `"Mon 14:32"`; older → `"2026-10-01"`
- [ ] **Step 3: Run** → FAIL. **Step 4: Implement.** **Step 5: Run** → PASS.
- [ ] **Step 6: Commit** `"Add missed-message and text helpers"`

### Task 5: Files, keyring and HTTP API client

**Files:**
- Create: `crates/core/src/files.rs`, `crates/core/src/secret.rs`, `crates/core/src/api.rs`
- Test: `crates/core/tests/api.rs` (wiremock)

**Interfaces:**
- Consumes: models, `Monitors`, `PAGE_LIMIT`
- Produces:
  - `struct Config { server_url: Url, client_id: i64, client_name: String, user_name: String }`
  - `struct State { last_read_id: Option<i64>, last_notified_id: Option<i64>, monitors: Monitors }` (Default)
  - `struct Paths { config_file: PathBuf, state_file: PathBuf }` with `fn Paths::from_env() -> Paths` and `fn Paths::in_dir(dir: &Path) -> Paths` (tests)
  - `fn load_config(p: &Paths) -> io::Result<Option<Config>>`, `fn save_config(p: &Paths, c: &Config) -> io::Result<()>`, `fn remove_config(p: &Paths) -> io::Result<()>`
  - `fn load_state(p: &Paths) -> State` (missing or corrupt file → `State::default()`, corrupt file renamed to `state.json.bad`), `fn save_state(p: &Paths, s: &State) -> io::Result<()>` (write temp file + rename)
  - `enum SecretError { Locked, Unavailable(String) }`; `async fn load_token(server: &Url) -> Result<Option<String>, SecretError>`; `async fn store_token(server: &Url, token: &str) -> Result<(), SecretError>`; `async fn delete_token(server: &Url) -> Result<(), SecretError>` — keyring item attributes `{"application": "gotify-tray", "server": <url string>}`, label `"Gotify tray token"`. Use oo7's D-Bus Secret Service API directly (not the file backend); check the default collection's `is_locked` and return `Locked` instead of unlocking.
  - `enum ApiError { Unauthorized, Forbidden, Http(u16, String), Network(String), Decode(String) }` (thiserror)
  - `struct GotifyClient` (Clone) with `fn new(base: Url, token: String) -> Self` and async methods: `version() -> VersionInfo`, `current_user() -> User`, `applications() -> Vec<Application>`, `messages_page(app: Option<i64>, since: Option<i64>) -> PagedMessages` (limit 200), `all_messages() -> Vec<Message>` (follows `since` until `paging.next` is empty), `messages_newer_than(id: i64) -> Vec<Message>` (stops paging once a page contains an id ≤ `id`; returns only ids > `id`, newest first), `delete_message(id)`, `delete_app_messages(app_id)`, `delete_all_messages()`, `delete_client(id)`, `image(path: &str) -> Vec<u8>`, `base() -> &Url`, `token() -> &str` — all return `Result<_, ApiError>`
  - Free functions: `async fn server_info(base: &Url) -> Result<GotifyInfo, ApiError>`, `async fn create_client(base: &Url, user: &str, password: &str, name: &str) -> Result<NewClient, ApiError>`
  - `fn endpoint(base: &Url, path: &str) -> Url` — joins under the base path (base `https://h/gotify` and `https://h/gotify/` both give `https://h/gotify/message`)
- HTTP status mapping: 401 → `Unauthorized`, 403 → `Forbidden`, other non-2xx → `Http(code, errorDescription or body)`.
- reqwest client: rustls, connect timeout 10 s, request timeout 30 s, user agent `gotify-tray/<version>`.

- [ ] **Step 1: Write failing tests** (`files.rs` unit tests with `tempfile`): `config_roundtrip`, `missing_config_is_none`, `state_roundtrip_with_monitors`, `corrupt_state_is_default_and_moved_aside`.
- [ ] **Step 2: Write failing tests** (`tests/api.rs`, wiremock):
  - `urls_respect_path_prefix`: `endpoint("https://h/gotify/", "message")` and without trailing slash → `https://h/gotify/message`
  - `sends_token_header`: mock requires header `X-Gotify-Key: T` → `applications()` OK
  - `all_messages_pages_with_since`: page 1 (200 msgs, `next` set, `since=101`) then request with `since=101` returns 2 msgs, `next` null → 202 messages
  - `newer_than_stops_early`: one page with ids 10..1, `messages_newer_than(7)` → ids `[10, 9, 8]`, only one request made
  - `maps_401_and_403`: 401 → `Unauthorized`; `delete_client` 403 → `Forbidden`
  - `create_client_uses_basic_auth`: mock requires `Authorization: Basic dTpw` (u:p) and JSON body `{"name":"Ubuntu tray (host)"}` → `NewClient` with token
- [ ] **Step 3: Run** `cargo test -p gotify-core` → FAIL. **Step 4: Implement.** **Step 5: Run** → PASS.
- [ ] **Step 6: Manual keyring check** (small `examples/keyring.rs`: store, load, delete a dummy token for `https://example.invalid`) → `load` returns the stored value; `secret-tool search application gotify-tray` shows it before delete and nothing after.
- [ ] **Step 7: Commit** `"Add config/state files, keyring and Gotify API client"`

### Task 6: WebSocket stream

**Files:**
- Create: `crates/core/src/stream.rs`
- Test: `crates/core/tests/stream.rs` (local tokio-tungstenite server on 127.0.0.1)

**Interfaces:**
- Consumes: `Message`, `ApiError`, `endpoint`, `STREAM_SILENCE_TIMEOUT`
- Produces:
  - `enum StreamEnd { Closed, Silent, Unauthorized, Error(String) }`
  - `struct MessageStream`; `async fn connect_stream(base: &Url, token: &str, silence: Duration) -> Result<MessageStream, ApiError>` (URL: `endpoint(base, "stream")` with `https→wss`, `http→ws`; header `X-Gotify-Key`; HTTP 401 during handshake → `ApiError::Unauthorized`)
  - `impl MessageStream { async fn next(&mut self) -> Result<Message, StreamEnd> }` — any received frame (including ping) resets the silence timer; no frame for `silence` → `Err(StreamEnd::Silent)`; pongs are answered automatically; frames that aren't valid message JSON are skipped (logged).

- [ ] **Step 1: Write failing tests:** `stream_yields_messages` (server sends 2 message JSON texts → 2 `Message`s, then close → `Closed`), `stream_reports_silence` (server stays silent; silence = 200 ms → `Silent` within 400 ms), `pings_keep_stream_alive` (server pings every 100 ms for 500 ms with silence = 200 ms → no `Silent` before the server closes), `handshake_401_is_unauthorized`, `skips_invalid_json`.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS.
- [ ] **Step 5: Verify Gotify's ping interval** in the Gotify 3.1 server docs/config (`GOTIFY_SERVER_STREAM_PINGPERIODSECONDS`, expected default 45 s). If it's larger than 45 s, set `STREAM_SILENCE_TIMEOUT` to 2× ping + 10 s and note it in the commit message.
- [ ] **Step 6: Commit** `"Add Gotify WebSocket stream"`

### Task 7: Local Gotify for integration tests

**Files:**
- Create: `tests/docker-compose.yml`, `tests/seed.sh`, `crates/core/tests/live_gotify.rs`

**Interfaces:**
- Produces: `tests/seed.sh` — starts `gotify/server:3.1.0` on `127.0.0.1:18080` (admin `admin`/`admin`), creates application `Uptime Kuma` via Basic auth, prints `APP_TOKEN=...`, and has a `send` subcommand: `tests/seed.sh send <priority> "<text>"` posting `title=Uptime-Kuma`. Integration tests read `GOTIFY_TEST_URL` and skip (pass with a printed note) when it isn't set.

- [ ] **Step 1: Write** compose file + seed script. Run `tests/seed.sh up` → prints an app token; `curl 127.0.0.1:18080/version` → `3.1.0`.
- [ ] **Step 2: Write failing test** `live_roundtrip` (`#[ignore]`-free but env-gated): `create_client` with admin/admin → `connect_stream` → send `"[web01] [🔴 Down] test"` with the app token → stream yields it with `parse_kuma` Down; `all_messages` contains it; `delete_message` removes it; `delete_client` on own client → `Forbidden` (proves the elevation assumption).
- [ ] **Step 3: Run** `GOTIFY_TEST_URL=http://127.0.0.1:18080 cargo test -p gotify-core --test live_gotify` → PASS (fix the client if not).
- [ ] **Step 4: Commit** `"Add local Gotify integration test setup"`

### Task 8: Desktop notifications (daemon)

**Files:**
- Create: `crates/daemon/src/notify.rs`

**Interfaces:**
- Consumes: `Message`, `delivery_for`, `display_title`, `parse_kuma`, `first_url`, `escape_markup`, `MissedNotice`, `Urgency`
- Produces:
  - `struct NotifyRequest { summary: String, body: String, urgency: Urgency, actions: Vec<(String, String)>, replaces_id: u32 }`
  - `enum Target { OpenWindow(Option<i64>), OpenLink(String) }`
  - `fn request_for_message(msg: &Message, app_name: Option<&str>, urgency: Urgency) -> NotifyRequest`
  - `fn request_for_summary(count: usize, urgency: Urgency, lines: &[String]) -> NotifyRequest`
  - `fn request_for_login_expired() -> NotifyRequest`
  - `trait NotifyBackend { async fn notify(&mut self, r: &NotifyRequest) -> u32; }`; `struct DbusBackend` implementing it, `async fn DbusBackend::new(conn: &zbus::Connection) -> zbus::Result<DbusBackend>`, plus `fn DbusBackend::signals(&self) -> impl Stream<Item = NotifierSignal>` where `enum NotifierSignal { Action(u32, String), Closed(u32) }`
  - `struct Notifier<B: NotifyBackend>` with `fn new(backend: B) -> Self`, `async fn show_message(&mut self, msg: &Message, app_name: Option<&str>)` (does nothing for `Delivery::Silent`), `async fn show_summary(&mut self, ..)`, `async fn show_login_expired(&mut self)`, `fn on_closed(&mut self, id: u32)`, `fn on_action(&mut self, id: u32, key: &str) -> Option<Target>`
- Exact text:
  - Kuma Down: summary `"🔴 {monitor} is down"`, body = details. Kuma Up: `"✅ {monitor} is up"`, body = details.
  - Other: summary = `display_title`, body = message.
  - Summary: `"{count} missed messages"`, body = lines joined by `\n`.
  - Login expired: `"Gotify login expired"`, body `"Open Settings from the tray menu to log in again."`
  - Body always `escape_markup`'d. Actions: `("default", "Open")` always; `("open-link", "Open link")` when `first_url` finds one. Summary's `default` → `OpenWindow(None)`.
- D-Bus call `org.freedesktop.Notifications.Notify(app_name="Gotify", replaces_id, app_icon="", summary, body, actions, hints, expire_timeout=-1)` with hints `urgency` (byte 0/1/2) and `desktop-entry = "gotify-tray"`.
- Replacement: Notifier keeps `down_by_monitor: HashMap<String, u32>`; a Kuma Up uses the stored id of that monitor's Down as `replaces_id` and removes it; a Kuma Down stores its returned id; `on_closed` drops the id from both maps.

- [ ] **Step 1: Write failing tests** (pure request builders + tracker logic with a fake id source):
  - `kuma_down_request`: summary `"🔴 web01 is down"`, urgency Critical, actions contain `default` only
  - `link_adds_action`: message `"Deploy failed https://ci/x"` → actions include `("open-link","Open link")`; `on_action(id,"open-link")` → `OpenLink("https://ci/x")`
  - `body_is_escaped`: message `"a < b & c"` → body `"a &lt; b &amp; c"`
  - `up_replaces_down`: tracker given Down for `web01` → returned id 7 stored; next Up for `web01` → `replaces_id == 7`; a second Up → `replaces_id == 0`
  - `closed_down_is_not_replaced`: Down id 7, `on_closed(7)`, Up → `replaces_id == 0`
  - `summary_text`: count 5 → `"5 missed messages"`
  - Tracker tests use a fake `NotifyBackend` that returns increasing ids and records requests.
- [ ] **Step 2: Run** `cargo test -p gotify-tray notify` → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS.
- [ ] **Step 5: Manual GNOME check** with a tiny `examples/notify_demo.rs`: Down notification stays until dismissed; Up for the same monitor replaces it and fades; with Do Not Disturb on, check whether the critical one shows. Write the observed DND behaviour into README "Known behaviour".
- [ ] **Step 6: Commit** `"Add desktop notifications"`

### Task 9: Tray icon and menu

**Files:**
- Create: `crates/daemon/src/icon.rs`, `crates/daemon/src/tray.rs`

**Interfaces:**
- Consumes: `MonitorRecord`, `since_label`
- Produces:
  - `enum IconKind { Ok, Down, Offline }`; `fn render(kind: IconKind, unread: bool, size: i32) -> ksni::Icon` (ARGB32, network byte order). Disc of 80 % of `size`, colours: Ok `#f6f5f4`, Down `#e01b24`, Offline `#77767b`; unread dot `#3584e4`, radius 20 % of size, centred at (80 %, 20 %). Provide sizes 22 and 44.
  - `struct DownEntry { monitor: String, message_id: i64, since: DateTime<Utc> }`
  - `struct TrayView { icon: IconKind, unread: bool, status_line: String, down: Vec<DownEntry> }` (Clone, PartialEq, Default = Offline + `"Starting…"`)
  - `enum TrayCommand { ShowWindow(Option<i64>), Settings, ClearMonitor(String), MenuOpened, Quit }`
  - `struct GotifyTray { view: TrayView, tx: tokio::sync::mpsc::UnboundedSender<TrayCommand> }` implementing `ksni::Tray` with `MENU_ON_ACTIVATE = true`, `id() = "gotify-tray"`, `title() = "Gotify"`, `tool_tip` = status line, `menu_about_to_show` → `MenuOpened`
  - `fn menu_model(view: &TrayView, now: DateTime<Local>) -> Vec<MenuEntry>` — a plain, testable description of the menu (`enum MenuEntry { Label(String), Separator, Item(String, TrayCommand), Sub(String, Vec<MenuEntry>) }`) that `menu()` converts to ksni items.
- Menu order: status line (disabled label) · separator · if any down: label `"Down right now"`, then per monitor a submenu `"{monitor} — down since {since_label}"` with `"Show message"` → `ShowWindow(Some(id))` and `"Clear"` → `ClearMonitor(name)` · separator · `"Show messages"` → `ShowWindow(None)` · `"Settings"` → `Settings` · `"Quit"` → `Quit`.

- [ ] **Step 1: Write failing tests:** `icon_center_colours` (centre pixel per kind), `icon_corner_transparent`, `unread_dot_blue` (pixel at (80 %, 20 %) is `#3584e4` only when unread), `icon_data_len` (= size²·4), `menu_without_down` (no "Down right now"), `menu_with_down` (label + submenu text `"web01 — down since 14:32"` with both actions).
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `"Add tray icon and menu"`

### Task 10: Daemon core — connection, sync, D-Bus service, single instance

**Files:**
- Create: `crates/daemon/src/app.rs`, `crates/daemon/src/backoff.rs`, `crates/daemon/src/service.rs`, `crates/daemon/src/signals.rs`, `crates/core/src/ipc.rs`
- Modify: `crates/daemon/src/main.rs`

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `ipc.rs`: constants from Global Constraints; zbus proxy `#[proxy(interface = "dev.gotifytray.GotifyTray1", default_service = "dev.gotifytray.GotifyTray", default_path = "/dev/gotifytray/GotifyTray")] trait GotifyTray { fn show_window(&self, message_id: i64) -> zbus::Result<()>; /* 0 = none */ fn resync(&self) -> zbus::Result<()>; fn reload_config(&self) -> zbus::Result<()>; fn mark_read(&self, up_to_id: i64) -> zbus::Result<()>; fn last_read(&self) -> zbus::Result<i64>; /* 0 = none */ }`
  - `backoff.rs`: `struct Backoff`; `fn next_delay(&mut self) -> Duration` (1, 2, 4, … capped at 60 s); `fn reset(&mut self)`
  - `app.rs`:
    - `enum Conn { NotConfigured, WaitingForKeyring, LoginExpired, Connecting, Connected, Reconnecting }`
    - `fn tray_view(conn: &Conn, server_host: Option<&str>, monitors: &Monitors, unread: usize) -> TrayView` — status lines: `"Connected to {host}"`, `"Reconnecting…"`, `"Connecting…"`, `"Not configured — open Settings"`, `"Waiting for keyring"`, `"Login expired — open Settings"`; icon: Offline unless Connected; Down if `down_list` non-empty (also while reconnecting, so a known outage stays visible); unread = `unread > 0`
    - `enum Event { Stream(Message), StreamEnded(StreamEnd), Synced(Result<SyncResult, ApiError>), Tray(TrayCommand), Notifier(NotifierSignal), Service(ServiceCall), WindowExited, Wake /* resume or network up */, Tick /* resync interval */ }`
    - `struct SyncResult { apps: Vec<Application>, messages: Vec<Message> }`
    - `struct App` owning `Paths`, `State`, `Option<Config>`, `Conn`, `Notifier`, tray `ksni::Handle`, `Backoff`, `apps: HashMap<i64, String>`, `message_ids: BTreeSet<i64>`, `pending: Vec<Message>` (stream messages received during a sync), `last_sync: Option<Instant>`; `async fn run(self, rx: mpsc::Receiver<Event>)`
  - `service.rs`: `struct Service { tx }` implementing the interface with `#[zbus::interface]`, forwarding as `ServiceCall { ShowWindow(Option<i64>), Resync, ReloadConfig, MarkRead(i64) }`; `last_read` answers from a shared `Arc<AtomicI64>` the App keeps updated.
  - `signals.rs`: `async fn watch(system: zbus::Connection, tx)` → `Event::Wake` on logind `PrepareForSleep(false)` and NetworkManager `StateChanged` to `70` (connected global).
- Behaviour (spec "Robustness", "Down right now", "Start, install, single instance"):
  1. `main`: tokio current-thread runtime; connect session bus; request name `dev.gotifytray.GotifyTray` with `DoNotQueue`; if taken → call `ShowWindow(0)` on the running instance and exit 0. Start tray, notifier, service, signals.
  2. Startup: load config → none ⇒ `NotConfigured`. Load token → `Locked` ⇒ `WaitingForKeyring`, retry every `KEYRING_RETRY` (only in this state); none ⇒ `NotConfigured`.
  3. Connect: open stream first, then sync (`applications` + `all_messages`). Stream messages arriving while a sync is running go to `pending`. When the sync finishes: `monitors = resync(..)`, `missed_notice(..)` → show Single/Summary, update `message_ids`, unread, save state; then drain `pending`, notifying only ids > `last_notified_id`.
  4. Live message: `apply_live`, notify (if id > `last_notified_id`), update `last_notified_id`, `message_ids`, unread, tray, save state.
  5. Stream end / network error → `Reconnecting`, wait `backoff.next_delay()` (cut short by `Wake`); `Unauthorized` (stream or API) → `LoginExpired`, one `show_login_expired`, **no reconnect** until `ReloadConfig`.
  6. Resync triggers: `Tick` every `RESYNC_INTERVAL`, `MenuOpened` (skip if last sync < `MENU_RESYNC_DEBOUNCE` ago), `WindowExited`, `Resync` call. Resync = sync only (stream stays).
  7. Window: `ShowWindow`/notification `OpenWindow`/tray items spawn `gotify-tray-window` from `current_exe()`'s directory with `--message <id>` or `--settings`; await the child in a task and send `WindowExited`. (GtkApplication forwards to an already-open window.) `OpenLink` → spawn `xdg-open <url>`.
  8. `ClearMonitor` → `monitors::clear`, save, update tray. `MarkRead(id)` → `last_read_id = max(old, id)`, save, update tray. `ReloadConfig` → drop connection, restart from step 2.
  9. `Quit` → exit cleanly (close stream, release bus name).

- [ ] **Step 1: Write failing tests** (`backoff.rs`, `app.rs` unit tests):
  - `backoff_doubles_and_caps`: 1, 2, 4, 8, 16, 32, 60, 60; after `reset` → 1
  - `tray_view_states`: Connected + one Down → icon Down, status `"Connected to gotify.example.com"`; Reconnecting with a Down → icon Down, `"Reconnecting…"`; NotConfigured → Offline
- [ ] **Step 2: Write failing integration tests** in `crates/daemon/tests/app_live.rs` (env-gated on `GOTIFY_TEST_URL`, like Task 7), driving `App` with a fake `NotifyBackend` that records requests and a temp `Paths`:
  - `missed_messages_summarised`: state `last_notified_id` = current max; send 3 non-silent messages while App is not running; start App → exactly one Summary request `"3 missed messages"`
  - `sync_buffers_stream_messages`: hold the sync (test hook that delays `all_messages` by 500 ms), send a message during the delay → it's notified exactly once
  - `unauthorized_stops_reconnecting`: delete the App's client via Basic auth while connected → exactly one login-expired request, `Conn::LoginExpired`, no further connection attempts within 5 s
  - `deleting_down_message_clears_on_resync`: send Down, wait, delete it via API, send `Resync` → down list empty
  - Make `App` testable: constructor takes `impl NotifyBackend`, a `Paths`, a token source closure instead of the keyring, and an optional tray handle (`None` in tests).
- [ ] **Step 3: Run** `cargo test -p gotify-tray` (with the local Gotify up) → FAIL. **Step 4: Implement.** **Step 5: Run** → PASS.
- [ ] **Step 6: Manual check** against the local Gotify: run `gotify-tray`, log in by writing config + keyring token with the Task 5 example, send Down/Up with `tests/seed.sh send`: tray turns red/normal, menu lists the monitor; second `gotify-tray` launch exits immediately; `systemctl suspend` and resume → reconnects within seconds (journal/log shows it); stop the Docker container → status `Reconnecting…`, start it → missed messages summarised.
- [ ] **Step 7: Commit** `"Add background service: connection, sync, tray and D-Bus"`

### Task 11: Window app — runtime and settings/login

**Files:**
- Create: `crates/window/src/rt.rs`, `crates/window/src/settings.rs`
- Modify: `crates/window/src/main.rs`

**Interfaces:**
- Consumes: `api::*`, `files::*`, `secret::*`, `ipc::GotifyTrayProxy`
- Produces:
  - `rt.rs`: `fn runtime() -> &'static tokio::runtime::Runtime` (multi-thread, 1 worker, `OnceLock`); `fn spawn_ui<T: Send + 'static>(fut: impl Future<Output = T> + Send + 'static, done: impl FnOnce(T) + 'static)` — runs `fut` on the runtime and calls `done` on the GTK main loop (via `glib::spawn_future_local` + `async_channel`).
  - `main.rs`: `adw::Application` id `dev.gotifytray.GotifyTray.Window`, flag `HANDLES_COMMAND_LINE`; args `--settings`, `--message <id>`; a second invocation is forwarded to the running window (raise it, open settings or jump to the message). Window process exits when its last window closes.
  - `settings.rs`: `fn open_settings(parent: &impl IsA<gtk::Widget>)` → `adw::PreferencesDialog`:
    - Logged out: entries Server URL, Username, Password (`adw::PasswordEntryRow`); buttons "Test connection" and "Log in".
    - Test: `server_info` → `"Gotify {version} — ready to log in"`; `local_auth == false` → `"This server only allows OIDC login, which isn't supported yet."`; errors shown inline in plain words.
    - Log in: `create_client(url, user, pass, "Ubuntu tray ({hostname})")` → `current_user` with the new token → `store_token` → `save_config` → `GotifyTrayProxy::reload_config()` (ignore if the daemon isn't running) → switch to logged-in view. Wrong credentials (401) → `"Wrong username or password."`
    - Logged in: `"Connected to {host} as {user}"`, button "Log out": `delete_client(client_id)`; on `Forbidden` show `"Remove the client “{client_name}” in Gotify's web UI (Clients) to revoke it fully."`; then `delete_token`, `remove_config`, `reload_config()`.
    - URL normalisation: trim, add `https://` when no scheme is given, reject anything that isn't http/https.

- [ ] **Step 1: Write failing tests** for the only pure part: `normalize_server_url` (`"gotify.example.com"` → `https://gotify.example.com/`; `" https://h/gotify/ "` → `https://h/gotify/`; `"ftp://x"` → error) and `client_name("laptop")` → `"Ubuntu tray (laptop)"`.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement** (tests pass) and build the dialog.
- [ ] **Step 4: Manual check** against local Gotify: `gotify-tray-window --settings` → test connection shows `Gotify 3.1.0 — ready to log in`; wrong password message; log in → client `Ubuntu tray (<hostname>)` appears in Gotify's web UI, `secret-tool search application gotify-tray` finds the token, config file has no token, running daemon turns to "Connected"; log out → note shown, token and config removed, daemon shows "Not configured".
- [ ] **Step 5: Commit** `"Add window app with settings and login"`

### Task 12: Message window

**Files:**
- Create: `crates/window/src/messages.rs`, `crates/window/src/row.rs`
- Modify: `crates/window/src/main.rs`

**Interfaces:**
- Consumes: `GotifyClient`, `connect_stream`, `parse_kuma`, `display_title`, `segments`, `escape_markup`, `relative_time`, `GotifyTrayProxy`, `spawn_ui`, `open_settings`
- Produces: `fn build_window(app: &adw::Application, focus_message: Option<i64>) -> adw::ApplicationWindow`; `fn row_markup(msg: &Message) -> String` (pure); `fn show_sidebar(app_count: usize) -> bool` (pure, `app_count > 1`)
- Layout (spec "Message window"): `adw::NavigationSplitView`; sidebar `gtk::ListBox` with "All messages" + one row per app (custom image via `GotifyClient::image`, unread count badge using `last_read()` from D-Bus); collapsed/hidden when `show_sidebar` is false. Content: `gtk::ListBox` in a `ScrolledWindow`; on `edge-reached` bottom load the next page (`since`). Header bar: "Delete all" (confirmation `adw::AlertDialog`: `"Delete all messages from {app}?"` / `"Delete all messages?"`, destructive) and a menu with "Settings".
- Row (`row.rs`): app name (only when >1 app) · `relative_time`; title: Kuma → `"🔴 {monitor}"` / `"✅ {monitor}"`, others `display_title`; body label with Pango markup from `row_markup` (escaped text, `<a href>` for links; Kuma body = details), wrapping, selectable; CSS class `error` marker for non-Kuma priority ≥ 8 and for Kuma Down; delete button `user-trash-symbolic` → `delete_message` → remove row; failures → `adw::Toast`. Links open via `gtk::UriLauncher`.
- Live: the window opens its own `connect_stream`; new messages are prepended (and to the right app filter); while the window is active, call `mark_read(newest_id)`.
- On open: load applications + first page, then `mark_read(newest_id)`. With `focus_message`, keep loading pages until that id is found (or history ends), scroll to it and highlight it briefly.
- Not configured / login expired / unreachable: `adw::StatusPage` with plain text and an "Open Settings" button (unreachable: "Can't reach {host}" + "Try again").

- [ ] **Step 1: Write failing tests:** `row_markup_escapes_and_links` (`"a<b https://x.y/z"` → `"a&lt;b <a href=\"https://x.y/z\">https://x.y/z</a>"`), `row_markup_kuma_uses_details`, `sidebar_hidden_for_one_app` (`show_sidebar(1) == false`, `show_sidebar(2) == true`).
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement** (tests pass) and build the window.
- [ ] **Step 4: Manual check** against local Gotify (seed ~250 messages across 2 apps so paging and the sidebar show): newest first; scrolling loads older messages; app filter works; delete one → gone in Gotify web UI; "Delete all" for one app; link opens in browser; new message appears live; opening from a tray "Down right now" entry jumps to that message; tray unread dot clears after opening; closing the window ends the `gotify-tray-window` process (check `pgrep`) and the daemon re-syncs.
- [ ] **Step 5: Commit** `"Add message window"`

### Task 13: Install, autostart, README and resource check

**Files:**
- Create: `install.sh`, `data/gotify-tray.desktop`, `README.md`

**Interfaces:**
- Produces: `./install.sh` (build release, install both binaries to `~/.local/bin`, desktop file to `~/.local/share/applications/gotify-tray.desktop` and `~/.config/autostart/gotify-tray.desktop` with `Exec=` set to the absolute binary path, run `update-desktop-database ~/.local/share/applications` if present); `./install.sh --uninstall` removes exactly those files (leaves config, state and keyring item, and says so).
- Desktop file: `Name=Gotify`, `Comment=Gotify notifications in the tray`, `Icon=preferences-system-notifications`, `Type=Application`, `Categories=Utility;Network;`, `StartupNotify=false`, `X-GNOME-Autostart-enabled=true`.

- [ ] **Step 1: Write** the files; `desktop-file-validate data/gotify-tray.desktop` → no errors.
- [ ] **Step 2: Run** `./install.sh` → app appears in the GNOME app menu; log out/in → tray icon appears without launching anything by hand; `./install.sh --uninstall` → files gone.
- [ ] **Step 3: Resource check** (daemon connected to local Gotify, window closed, 5 min idle): `grep VmRSS /proc/$(pgrep -x gotify-tray)/status` → **≤ 15360 kB**; sum of `voluntary_ctxt_switches` over all threads before/after 5 min → increase < 100. `ldd ~/.local/bin/gotify-tray | grep -c gtk` → `0`. If RSS is over budget: check TLS crypto provider, tokio features and allocator before changing the design; report numbers either way.
- [ ] **Step 4: Read-only check against the real server** using `~/.config/gotify-check` (GET only): run `gotify-core`'s `examples/inspect.rs` (new, tiny: `applications`, `all_messages`, `resync`, `down_list`) and confirm it parses all 152+ messages, finds the monitors and prints the current down list (names only, no message bodies).
- [ ] **Step 5: README:** what it is, requirements (GNOME + AppIndicator extension, keyring), install/uninstall, login, how Uptime Kuma messages are treated, priority table, files and what is stored where, known behaviour (DND result from Task 8), how to run tests (`tests/seed.sh up`, `GOTIFY_TEST_URL=...`).
- [ ] **Step 6: Commit** `"Add installer, desktop entry and README"`
