#!/usr/bin/env bash
# Builds Chorong.app (macOS): the viewer (chorong-app) and the command-line tool (cho) side by side in
# Contents/MacOS, so `cho .` starts the viewer next to it with the app's icon; the system can
# hand it chorong:// links (CFBundleURLTypes in app/macos/Info.plist); the icon is
# app/icons/<SET>/macos/Chorong.icns. The bundle is signed ad hoc (no developer account), as Apple
# silicon needs, and ready to copy.
# Usage: scripts/bundle-macos.sh [--icon lantern|etched-m] [OUT_DIR]
#   (defaults: the lantern icon, target/release)
# Then move Chorong.app to /Applications (or open it once) so Launch Services registers it, and put
# its Contents/MacOS folder on your PATH (the script prints the line).
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
icon=lantern; out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --icon) [[ $# -ge 2 ]] || { echo "bundle-macos: --icon needs a set name" >&2; exit 2; }
            icon="$2"; shift 2 ;;
    -*) echo "bundle-macos: unknown option $1" >&2; exit 2 ;;
    *) out="$1"; shift ;;
  esac
done
out="${out:-$repo/target/release}"
icns="$repo/app/icons/$icon/macos/Chorong.icns"
[[ -f "$icns" ]] || { echo "bundle-macos: no icon set '$icon' (have: $(ls "$repo/app/icons" | grep -v -E '\.(md|js)$' | xargs))" >&2; exit 2; }
# GPUI compiles its Metal shaders at build time with Apple's `metal` compiler, which comes with
# Xcode's Metal Toolchain, not the Command Line Tools: say so before a long build fails.
if ! xcrun -f metal >/dev/null 2>&1; then
  echo "bundle-macos: no Metal compiler (xcrun metal). Install Xcode, select it, add its Metal Toolchain:" >&2
  echo "bundle-macos:   sudo xcode-select -s /Applications/Xcode.app" >&2
  echo "bundle-macos:   xcodebuild -downloadComponent MetalToolchain" >&2
  exit 2
fi
cargo build --release -p chorong-app -p chorong --manifest-path "$repo/Cargo.toml"
built="${CARGO_TARGET_DIR:-$repo/target}/release"
bundle="$out/Chorong.app/Contents"
rm -rf "$out/Chorong.app"
mkdir -p "$bundle/MacOS" "$bundle/Resources"
cp "$built/chorong-app" "$built/cho" "$bundle/MacOS/"
cp "$repo/app/macos/Info.plist" "$bundle/Info.plist"
cp "$icns" "$bundle/Resources/Chorong.icns"
# Finder and the Dock cache icons by bundle path; touching the bundle makes them look again.
touch "$out/Chorong.app"
# Sealed last, after every file is in place: an ad-hoc signature carries no identity.
codesign --force --deep --sign - "$out/Chorong.app"
echo "built $out/Chorong.app (icon: $icon), with cho and chorong-app in Contents/MacOS"
echo "next: move it to /Applications, then put the cho command on your PATH:"
echo "  echo 'export PATH=\"/Applications/Chorong.app/Contents/MacOS:\$PATH\"' >> ~/.zshrc"
