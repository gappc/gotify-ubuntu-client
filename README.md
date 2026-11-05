# gotify-tray

A small Gotify client for Ubuntu (GNOME). It stays connected to one Gotify
server, shows desktop notifications, and puts an icon in the top bar that
turns red while an Uptime Kuma monitor is down.

- **Background service** (`gotify-tray`): connection, notifications, tray
  icon. Small and always running; it doesn't load GTK.
- **Message window** (`gotify-tray-window`): message list and settings.
  Starts when you open it and exits when you close it.

## Requirements

- Ubuntu with GNOME and the "Ubuntu AppIndicators" extension (enabled by
  default on Ubuntu) for the tray icon
- GNOME keyring (stores the login token)
- To build: Rust 1.99+, `libgtk-4-dev`, `libadwaita-1-dev`
- Gotify server 3.x with username/password login (OIDC isn't supported yet)

## Install

```sh
./install.sh              # build and install for your user
./install.sh --uninstall  # remove it again
```

This installs both programs to `~/.local/bin`, adds "Gotify" to the app menu
and starts it automatically when you log in. Start it right away with
`~/.local/bin/gotify-tray &`.

## Log in

Tray icon → **Settings**. Enter the server address, username and password,
then **Log in**. Like the Android app, this creates a client named
`Ubuntu tray (<hostname>)` in Gotify. The token goes into the GNOME keyring;
the password is not stored.

**Log out** removes the token from this computer. Gotify 3 doesn't let the
app delete its own client without a fresh password login, so remove the
client under *Clients* in Gotify's web UI to revoke it fully.

## Notifications

Uptime Kuma messages (`[monitor] [🔴 Down] …` / `[monitor] [✅ Up] …`) are
recognised regardless of their priority:

| Message | Notification |
|---|---|
| Kuma **Down** | stays on screen until dismissed (critical) |
| Kuma **Up** | replaces that monitor's Down notification, then fades |
| Other, priority 8–10 | stays on screen |
| Other, priority 4–7 | normal, fades |
| Other, priority 1–3 | no banner, only in GNOME's notification list |
| Other, priority 0 | no notification, only in the message window |

Clicking a notification opens the message window at that message; messages
with a link get an **Open link** button. Messages that arrive while the app
is disconnected (suspend, network loss, app not running) come as one summary,
e.g. "5 missed messages". On the very first start, old messages are not
notified.

## Tray icon

- light: connected, nothing down
- **red**: at least one Uptime Kuma monitor is down right now
- grey: not connected, not logged in, or waiting for the keyring
- blue dot: unread messages

The menu lists the monitors that are down and since when. **Clear** hides a
monitor that will never report "Up" again (deleted or renamed in Kuma) until
it sends a new message. Deleting a monitor's Down message also clears it.
The app checks with the server when the menu opens, when the message window
closes, and every 15 minutes.

## Files

| What | Where |
|---|---|
| Server address, client id | `~/.config/gotify-tray/config.toml` |
| Read/notified markers, monitor states | `~/.local/state/gotify-tray/state.json` |
| Login token | GNOME keyring (`application=gotify-tray`) |

## Known behaviour

- Whether critical notifications also show while Do Not Disturb is on
  depends on GNOME; not yet verified on GNOME 50.

## Development

```sh
cargo test --workspace                 # unit tests
tests/seed.sh up                       # local Gotify 3.1.0 in Docker on 127.0.0.1:18080
GOTIFY_TEST_URL=http://127.0.0.1:18080 cargo test --workspace   # + integration tests
tests/seed.sh send 8 "[web01] [🔴 Down] test"                  # send a Kuma-style message
tests/seed.sh down                     # stop and discard the test server
```

Integration tests never touch a real server. `examples/inspect.rs` does a
read-only check against a real one (`GOTIFY_URL`, `GOTIFY_TOKEN`).
