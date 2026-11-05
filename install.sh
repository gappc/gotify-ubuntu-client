#!/usr/bin/env bash
# Build and install gotify-tray for the current user.
#   ./install.sh              build (release) and install
#   ./install.sh --uninstall  remove the installed files
set -euo pipefail
cd "$(dirname "$0")"

BIN_DIR="$HOME/.local/bin"
APP_ID=dev.gotifytray.GotifyTray.Window
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_DIR="$DATA_DIR/applications"
ICON_DIR="$DATA_DIR/icons/hicolor"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
UNIT_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
SYSTEMCTL="${SYSTEMCTL:-systemctl}" # overridable for tests
# The menu entry is named after the window's app ID so GNOME shows its icon.
FILES=("$BIN_DIR/gotify-tray" "$BIN_DIR/gotify-tray-window" "$APP_DIR/$APP_ID.desktop"
       "$ICON_DIR/scalable/apps/$APP_ID.svg" "$UNIT_DIR/gotify-tray.service")
# Used by earlier versions (autostart is now the systemd user service).
OLD_FILES=("$APP_DIR/gotify-tray.desktop" "$AUTOSTART_DIR/gotify-tray.desktop")

if [ "${1:-}" = "--uninstall" ]; then
  "$SYSTEMCTL" --user disable --now gotify-tray.service 2>/dev/null || true
  pkill -f "^$BIN_DIR/gotify-tray\$" 2>/dev/null || true
  pkill -f "^$BIN_DIR/gotify-tray-window( |\$)" 2>/dev/null || true
  rm -f "${FILES[@]}" "${OLD_FILES[@]}"
  "$SYSTEMCTL" --user daemon-reload 2>/dev/null || true
  echo "Removed: ${FILES[*]}"
  echo "Kept your settings (~/.config/gotify-tray), state (~/.local/state/gotify-tray) and the token in the GNOME keyring."
  echo "Log out in Settings first if you also want those gone."
  exit 0
fi

cargo build --release
install -Dm755 target/release/gotify-tray "$BIN_DIR/gotify-tray"
install -Dm755 target/release/gotify-tray-window "$BIN_DIR/gotify-tray-window"
install -Dm644 "data/icons/$APP_ID.svg" "$ICON_DIR/scalable/apps/$APP_ID.svg"
mkdir -p "$APP_DIR" "$UNIT_DIR"
sed "s|^Exec=.*|Exec=$BIN_DIR/gotify-tray|" "data/$APP_ID.desktop" > "$APP_DIR/$APP_ID.desktop"
sed "s|^ExecStart=.*|ExecStart=$BIN_DIR/gotify-tray|" data/gotify-tray.service > "$UNIT_DIR/gotify-tray.service"
rm -f "${OLD_FILES[@]}"
command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t -f "$ICON_DIR" 2>/dev/null || true
# An open message window would keep showing the old version: close it and
# open it again (the new version) once the tray has been restarted.
WINDOW="^$BIN_DIR/gotify-tray-window( |\$)"
REOPEN_WINDOW=false
if pgrep -f "$WINDOW" >/dev/null; then
  pkill -f "$WINDOW" || true
  REOPEN_WINDOW=true
fi

# Run the tray as a systemd user service: started with the desktop session and
# restarted if it ever crashes. A copy started by hand (or by an older version's
# autostart entry) is stopped first so only the service's copy runs.
RUNNING="^$BIN_DIR/gotify-tray\$"
"$SYSTEMCTL" --user daemon-reload
if ! "$SYSTEMCTL" --user is-active --quiet gotify-tray.service && pgrep -f "$RUNNING" >/dev/null; then
  pkill -f "$RUNNING" || true
  for _ in $(seq 20); do pgrep -f "$RUNNING" >/dev/null || break; sleep 0.25; done
fi
"$SYSTEMCTL" --user enable gotify-tray.service >/dev/null 2>&1
"$SYSTEMCTL" --user restart gotify-tray.service
echo "Installed and (re)started gotify-tray (systemd user service gotify-tray.service)."
if $REOPEN_WINDOW; then
  setsid -f "$BIN_DIR/gotify-tray-window" >/dev/null 2>&1
  echo "Reopened the message window with the new version."
fi
echo "It starts with your desktop session and is restarted automatically if it crashes."
