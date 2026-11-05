# gotify-tray

A small Gotify client for Ubuntu (GNOME). It stays connected to one Gotify
server, shows desktop notifications, and puts an icon in the top bar that
turns red while an Uptime Kuma monitor is down.

- **Background service** (`gotify-tray`): connection, notifications, tray
  icon. Small (about 8 MB of memory) and always running; it doesn't load GTK.
  Runs as a systemd user service.
- **Message window** (`gotify-tray-window`): message list, search and
  settings. Starts when you open it and exits when you close it.

## Requirements

- Ubuntu with GNOME and the "Ubuntu AppIndicators" extension (enabled by
  default on Ubuntu) for the tray icon
- GNOME keyring (stores the login token)
- To build: Rust 1.99+, `libgtk-4-dev`, `libadwaita-1-dev`
- Gotify server 3.x
- Optional: an Uptime Kuma status page (see below)

## Install

```sh
./install.sh              # build, install and start (or restart) for your user
./install.sh --uninstall  # stop and remove it again
```

This installs both programs to `~/.local/bin`, adds "Gotify" (with its icon)
to the app menu, and runs the tray as the **systemd user service**
`gotify-tray.service`:

- it starts with your desktop session and stops when you log out,
- it's restarted automatically about 5 seconds after a crash,
- **Quit** in the tray menu stops it until your next login.

Running `./install.sh` again after an update restarts the service with the new
version, and closes and reopens an open message window so it shows the new
version too. Uninstalling keeps your settings, state and the keyring token
(log out in Settings first if you want those gone).

## Running and troubleshooting

| What | Command |
|---|---|
| Is it running, and since when? | `systemctl --user status gotify-tray` |
| Show its log | `journalctl --user -u gotify-tray` |
| Follow the log live | `journalctl --user -u gotify-tray -f` |
| Log since the last boot | `journalctl --user -u gotify-tray -b` |
| Start it again after "Quit" | `systemctl --user start gotify-tray` |
| Restart it | `systemctl --user restart gotify-tray` |
| Stop it | `systemctl --user stop gotify-tray` |
| Don't start it at login | `systemctl --user disable gotify-tray` |
| Start it at login again | `systemctl --user enable gotify-tray` |
| Open the message window | `~/.local/bin/gotify-tray` (while it runs, a second start opens the window) |

The log shows connection problems (e.g. "stream ended", "can't reach the
server"), status page errors and keyring problems. Tip: a GNOME custom
keyboard shortcut (Settings → Keyboard → Custom Shortcuts) running
`~/.local/bin/gotify-tray` opens your messages with one key press.

## Log in

Tray icon → **Settings**.

**With a client token (default, recommended):** in Gotify's web UI open
*Clients*, create a client (e.g. `ubuntu-tray`), copy its token, and paste it
together with the server address. The app never sees your password. Use a
*client* token, not an app token (app tokens can only send messages). This
also works on servers that only allow OIDC login.

**With your password:** choose "Log in with password instead". Like the
Android app, this creates a client named `Ubuntu tray (<hostname>)` in Gotify;
the password itself is not stored.

Either way the token goes into the GNOME keyring, never into a file.

**Log out** removes the token from this computer. To revoke it fully, delete
the client under *Clients* in Gotify's web UI (Gotify 3 doesn't let the app
delete it without a fresh password login).

## Uptime Kuma status page (optional, recommended)

Without it, the tray only knows what Kuma *sends*: if Kuma itself stops, the
icon would stay green. With a status page, the tray asks Kuma directly every
15 minutes (and when you open the menu, at most once a minute), so green means
"every monitor is up" and the menu shows e.g. "Connected — 14 monitors up".
The status page then decides what's down, and also covers monitors that don't
send notifications to Gotify. A Kuma message arriving through Gotify still
changes the icon and lists immediately (it wins over an older status page
answer), and the tray re-checks the status page about a minute later to
confirm.

1. In Kuma: **Status Pages → New Status Page**, choose a hard-to-guess slug,
   add all monitors, save. Status pages need no login, so keep Kuma private
   (e.g. on Tailscale) or use an unguessable slug.
2. In the app: **Settings → Uptime Kuma** → paste the page's address
   (`http://kuma:3001/status/your-slug`) → **Check and save**.

**Data use:** about 6–7 MB per day in total (fine on a mobile hotspot). Each
status page check is about 60 KB (Kuma always sends the last 100 checks per
monitor) and happens every 15 minutes; with a status page the tray also skips
its periodic re-read of the Gotify history. Nothing is loaded while the
computer is suspended.

Monitors that don’t send to Gotify still turn the tray
red, but give no pop-up; add your Gotify notification to them in Kuma
(*Edit monitor → Notifications*) if you want pop-ups and phone alerts too.

## Tray icon and menu

| Icon | Meaning |
|---|---|
| green | connected, nothing down (confirmed by Kuma if a status page is set up) |
| **red** | at least one monitor is down right now |
| **amber** | messages arrive, but the Kuma status page can't be reached twice in a row |
| grey | not connected, not logged in, or waiting for the keyring |
| blue dot | unread messages |

The menu shows the status line, the monitors that are **down right now** (and
since when), **Muted** monitors, **All monitors** (with a status page: every
monitor with its state and 24-hour uptime, e.g. "✅ api — up · 99.98 %";
clicking one, or *Open status page*, opens Kuma's page in your browser), and
*Show messages*, *Settings* and *Quit*. Uptime is rounded down, so it only
shows 100 % when every check succeeded; monitors that check in only a few
times a day can show low values after a single failed check.
Each down monitor offers *Show message* (if Gotify has one) and the mute
choices.

Without a status page, outages are worked out from the Gotify messages, and
each entry also has **Clear** for a monitor that will never report "Up" again
(deleted or renamed in Kuma). Deleting a monitor's Down message also clears it.
The app then re-checks with Gotify when the menu opens, when the message window
closes, after deleting in the window, and every 15 minutes (the 15-minute
re-check is skipped when a status page is set up).

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

If the app can't reach Gotify for **5 minutes**, it shows "Can't reach
Gotify" with the last error; it stays on screen, since no messages arrive
meanwhile. When the connection is back it's replaced by "Connected to Gotify
again", followed by the usual missed-messages summary. Time while the computer
is suspended doesn't count.

## Mute

Too noisy? Mute a monitor **for 1 day**, **for 1 week** or **until unmuted**:
from the tray menu (each entry under "Down right now") or with the bell button
on a message in the window (Kuma messages mute their monitor, other messages
their whole app). A muted monitor shows no pop-ups and doesn't turn the tray
icon red; it's listed under **Muted** in the tray menu with its state and when
the mute ends, and its messages stay in the window. If a mute ends while the
monitor is still down, you get its Down notification again. Mutes are stored on
this computer only (the tray service must be running to change them).

## Message window and search

Shows all messages, newest first; with more than one Gotify app a sidebar
filters by app. Delete single messages (trash button) or all of an app
(header button); deletions happen on the server, so they disappear on your
phone too. Links open in your browser. **Ctrl+W** closes the window.

**Search:** press **Ctrl+F**, click the search icon, or just start typing.
Every word must appear (upper/lower case doesn't matter) in the title, the
text, the monitor name or the app name, e.g. `lynq prod` or `backup`.
**Down only** keeps the messages shown in red: Uptime Kuma Down messages and
other high-priority ones. Search covers the full history of the selected app;
close the search bar (or press Esc) to see everything again.

## Files

| What | Where |
|---|---|
| Server address, user, status page address | `~/.config/gotify-tray/config.toml` |
| Read/notified markers, monitor states, mutes | `~/.local/state/gotify-tray/state.json` |
| Login token | GNOME keyring (`application=gotify-tray`) |
| Service definition | `~/.config/systemd/user/gotify-tray.service` |
| Programs | `~/.local/bin/gotify-tray`, `~/.local/bin/gotify-tray-window` |
| App menu entry and icon | `~/.local/share/applications/`, `~/.local/share/icons/hicolor/` |

## Known behaviour

- Kuma **Down** notifications stay on screen until dismissed (verified on
  GNOME 50).
- Whether they also show while Do Not Disturb is on depends on GNOME; not yet
  verified.

## Development

```sh
cargo test --workspace                 # unit tests
tests/seed.sh up                       # local Gotify 3.1.0 in Docker on 127.0.0.1:18080
GOTIFY_TEST_URL=http://127.0.0.1:18080 cargo test --workspace   # + integration tests
tests/seed.sh send 8 "[web01] [🔴 Down] test"                  # send a Kuma-style message
tests/seed.sh down                     # stop and discard the test server
```

Integration tests never touch a real server. To run test copies next to your
own, set `GOTIFY_TRAY_WINDOW_APP_ID=dev.gotifytray.TestWindow` and
`GOTIFY_TRAY_BUS_NAME=dev.gotifytray.Test` (plus separate `XDG_CONFIG_HOME` /
`XDG_STATE_HOME`), so they never talk to your running tray. `install.sh`
honours `SYSTEMCTL` (e.g. a stub) for testing the installer.

Read-only checks against real servers:

```sh
GOTIFY_URL=… GOTIFY_TOKEN=… cargo run -p gotify-core --example inspect
cargo run -p gotify-core --example kuma -- http://kuma:3001/status/your-slug
```
