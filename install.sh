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
# The menu entry is named after the window's app ID so GNOME shows its icon.
FILES=("$BIN_DIR/gotify-tray" "$BIN_DIR/gotify-tray-window" "$APP_DIR/$APP_ID.desktop"
       "$ICON_DIR/scalable/apps/$APP_ID.svg" "$AUTOSTART_DIR/gotify-tray.desktop")
OLD_FILES=("$APP_DIR/gotify-tray.desktop") # names used by earlier versions

if [ "${1:-}" = "--uninstall" ]; then
  pkill -f "^$BIN_DIR/gotify-tray\$" 2>/dev/null || true
  pkill -f "^$BIN_DIR/gotify-tray-window( |\$)" 2>/dev/null || true
  rm -f "${FILES[@]}" "${OLD_FILES[@]}"
  echo "Removed: ${FILES[*]}"
  echo "Kept your settings (~/.config/gotify-tray), state (~/.local/state/gotify-tray) and the token in the GNOME keyring."
  echo "Log out in Settings first if you also want those gone."
  exit 0
fi

cargo build --release
install -Dm755 target/release/gotify-tray "$BIN_DIR/gotify-tray"
install -Dm755 target/release/gotify-tray-window "$BIN_DIR/gotify-tray-window"
install -Dm644 "data/icons/$APP_ID.svg" "$ICON_DIR/scalable/apps/$APP_ID.svg"
mkdir -p "$APP_DIR" "$AUTOSTART_DIR"
sed "s|^Exec=.*|Exec=$BIN_DIR/gotify-tray|" "data/$APP_ID.desktop" > "$APP_DIR/$APP_ID.desktop"
sed "s|^Exec=.*|Exec=$BIN_DIR/gotify-tray|" "data/$APP_ID.desktop" > "$AUTOSTART_DIR/gotify-tray.desktop"
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

RUNNING="^$BIN_DIR/gotify-tray\$"
if pgrep -f "$RUNNING" >/dev/null; then
  # Replace the running (old) version with the new one.
  pkill -f "$RUNNING" || true
  for _ in $(seq 20); do pgrep -f "$RUNNING" >/dev/null || break; sleep 0.25; done
  setsid -f "$BIN_DIR/gotify-tray" >/dev/null 2>&1
  echo "Installed and restarted the running gotify-tray."
else
  echo "Installed. Start it now with: $BIN_DIR/gotify-tray &"
fi
if $REOPEN_WINDOW; then
  setsid -f "$BIN_DIR/gotify-tray-window" >/dev/null 2>&1
  echo "Reopened the message window with the new version."
fi
echo "It also starts automatically when you log in."
