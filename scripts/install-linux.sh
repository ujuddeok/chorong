#!/usr/bin/env bash
# Registers chorong-app as the handler for chorong:// links for the current user (Linux).
# Usage: scripts/install-linux.sh [PATH_TO_CHORONG_APP]   (default: chorong-app on PATH)
# Writes ~/.local/share/applications/chorong.desktop with an absolute Exec path, then makes it
# the default for x-scheme-handler/chorong. Touches nothing outside the user's own settings.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
app="${1:-$(command -v chorong-app || true)}"
[ -n "$app" ] && [ -x "$app" ] || { echo "chorong-app not found; build it (cargo build --release -p chorong-app) and pass its path" >&2; exit 2; }
app="$(cd "$(dirname "$app")" && pwd)/$(basename "$app")"
dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p "$dir"
# Quoted only when needed: some launchers (xdg-open's generic mode) don't parse quotes.
case "$app" in *[[:space:]]*) exec_path="\"$app\"" ;; *) exec_path="$app" ;; esac
sed "s|^Exec=chorong-app %u$|Exec=$exec_path %u|" "$repo/app/linux/chorong.desktop" >"$dir/chorong.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$dir" || true
xdg-mime default chorong.desktop x-scheme-handler/chorong
echo "chorong:// links now open with $app (handler: $(xdg-mime query default x-scheme-handler/chorong))"
