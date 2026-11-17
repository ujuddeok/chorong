# App icons

Two icon sets. The macOS bundle uses one (lantern by default); see "Wiring" below.

| Set | Design | Status |
| --- | --- | --- |
| `lantern/` | A cheongsachorong (Korean silk lantern) on night-blue glass etched with chilbo circles, a gold lotus crest lit from inside | Chosen |
| `etched-m/` | An M and a star cut through smoked glass, lit by a lantern behind | Backup |

Each set holds:

| File | What |
| --- | --- |
| `<set>.svg` | The master, 1024 px on the macOS icon grid (an 824 px tile with 100 px of margin). No fonts or external references. |
| `<set>-small.svg` | `lantern/` only: a simplified drawing used at 32 px and below, where the master's detail turns to noise |
| `macos/Chorong.icns` | 16 to 512 pt, each at @1x and @2x |
| `windows/chorong.ico` | 16, 24, 32, 48, 64, 128 and 256 px (PNG entries) |
| `linux/hicolor/<N>x<N>/apps/chorong.png` | 16, 22, 24, 32, 48, 64, 128, 256 and 512 px |

macOS keeps the grid margin. Windows and Linux crop most of it (viewBox `92 92 840 840`), so the
tile fills small sizes.

## Regenerating

Edit the SVGs, then run (needs Playwright's Chromium, preinstalled in cloud sessions; ImageMagick,
if present, recompresses the PNGs losslessly):

```sh
NODE_PATH="$(npm root -g)" node app/icons/render-icons.js
```

Commit the SVGs and the outputs together. Keep the PNGs lossless: palette quantisation shows as
grain in the glow and the silk.

## Wiring

- **macOS (done):** `scripts/bundle-macos.sh [--icon SET]` copies `<SET>/macos/Chorong.icns` to
  `Contents/Resources/Chorong.icns`; `app/macos/Info.plist` has `CFBundleIconFile` = `Chorong`.

Proposed, not done:

- **Linux:** `scripts/install-linux.sh` copies each `linux/hicolor/<N>x<N>/apps/chorong.png` to
  `${XDG_DATA_HOME:-~/.local/share}/icons/hicolor/<N>x<N>/apps/chorong.png` and runs
  `gtk-update-icon-cache` if present; `app/linux/chorong.desktop` gains `Icon=cho`.
- **Windows:** embed `windows/chorong.ico` as the executable's icon resource when a Windows build exists.
- **Window icon at run time** (title bar, taskbar on Linux and Windows): depends on what GPUI
  exposes; not checked.
