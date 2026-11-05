#!/usr/bin/env bash
# Build and install gotify-tray for the current user.
#   ./install.sh              build (release) and install
#   ./install.sh --uninstall  remove the installed files
set -euo pipefail
cd "$(dirname "$0")"

BIN_DIR="$HOME/.local/bin"
APP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
FILES=("$BIN_DIR/gotify-tray" "$BIN_DIR/gotify-tray-window" "$APP_DIR/gotify-tray.desktop" "$AUTOSTART_DIR/gotify-tray.desktop")

if [ "${1:-}" = "--uninstall" ]; then
  pkill -f "^$BIN_DIR/gotify-tray\$" 2>/dev/null || true
  rm -f "${FILES[@]}"
  echo "Removed: ${FILES[*]}"
  echo "Kept your settings (~/.config/gotify-tray), state (~/.local/state/gotify-tray) and the token in the GNOME keyring."
  echo "Log out in Settings first if you also want those gone."
  exit 0
fi

cargo build --release
install -Dm755 target/release/gotify-tray "$BIN_DIR/gotify-tray"
install -Dm755 target/release/gotify-tray-window "$BIN_DIR/gotify-tray-window"
for dir in "$APP_DIR" "$AUTOSTART_DIR"; do
  mkdir -p "$dir"
  sed "s|^Exec=.*|Exec=$BIN_DIR/gotify-tray|" data/gotify-tray.desktop > "$dir/gotify-tray.desktop"
done
command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" 2>/dev/null || true
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
echo "It also starts automatically when you log in."
