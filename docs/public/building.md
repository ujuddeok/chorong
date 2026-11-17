# Building

```bash
cargo build -p chorong        # headless CLI, no GUI dependencies
cargo build -p chorong-app        # GPUI viewer
cargo test --workspace
cargo run -p cho-layout-lab   # layout prototype (internal), same packages as the viewer
```

From crates.io, without a checkout: `cargo install chorong` installs the `cho` command and
`cargo install chorong-app` the viewer (it needs the Linux packages below too).

`cargo test --workspace` links the viewer, so it needs the Linux packages below. To test
everything else without any GUI packages: `cargo test --workspace --exclude chorong-app`.

Rust stable (see `rust-toolchain.toml`), edition 2024.

## Linux packages for the viewer

```bash
sudo apt-get install -y libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev clang pkg-config
```

Running the viewer on Linux also needs a Vulkan driver. Headless machines (CI, containers)
can use Mesa's software renderer:

```bash
sudo apt-get install -y mesa-vulkan-drivers libvulkan1 xvfb
xvfb-run -a target/debug/chorong-app .
```

Without it the viewer exits with `NoSupportedDeviceFound`.

macOS needs Xcode's command-line tools (`xcode-select --install`). The viewer's GPU shaders
are compiled at build time by Apple's `metal` compiler, which comes with Xcode's Metal
Toolchain, not the command-line tools: with full Xcode installed, add it with
`xcodebuild -downloadComponent MetalToolchain` (select it first with
`sudo xcode-select -s /Applications/Xcode.app` if `xcode-select -p` names the command-line
tools). Without it, a build of `chorong-app` stops with `unable to find utility "metal"`, and
`scripts/bundle-macos.sh` says what to install before it starts. The CLI and library crates
need no system packages on any platform.

## Cold builds

A cold `chorong-app` build compiles GPUI and about 800 registry crates, which takes ten minutes
or more. Later builds are incremental.

## Fuzzing

The parsers that read untrusted input (source files, `chorong://` links, themes, investigations)
have cargo-fuzz targets in `fuzz/`: `cd fuzz && cargo +nightly fuzz run <target>`. Seeds are in
`fuzz/seeds/<target>/`.

## Registering Chorong for `chorong://` links

So that clicking a `chorong://` link (in a graph page, a chat, a report) opens Chorong.

**Linux.** `app/linux/chorong.desktop` declares `MimeType=x-scheme-handler/chorong`. Install it for
your user with an absolute path to the viewer:

```bash
cargo build --release -p chorong-app
scripts/install-linux.sh "$PWD/target/release/chorong-app"
xdg-mime query default x-scheme-handler/chorong     # chorong.desktop
xdg-open 'chorong://open?repo=local:~/code/chorong&path=README.md'
```

The script writes `~/.local/share/applications/chorong.desktop` and runs `xdg-mime default`
(it needs `xdg-utils`). Only one viewer runs per user: a launch with a link hands it to the
running viewer over `$XDG_RUNTIME_DIR/chorong/viewer.sock` (or `/tmp/chorong-UID/viewer.sock`)
and exits.

**macOS.** `scripts/bundle-macos.sh` builds `target/release/Chorong.app` with the viewer
(`chorong-app`) and the command-line tool (`cho`) side by side in `Contents/MacOS`,
`app/macos/Info.plist`, which declares the `cho` URL scheme (`CFBundleURLTypes`), and the
lantern app icon (`--icon etched-m` uses the other set in `app/icons/`), and signs the bundle
ad hoc (no identity in it). Putting `Contents/MacOS` on your PATH gives you `cho`, and `cho .`
starts the viewer beside it with the app's icon. Move it to
`/Applications` (or open it once) so Launch Services registers it. macOS then hands links to
the running app, which receives them through GPUI's open-URL callback.

## Build profile

`Cargo.toml` builds dependencies at `opt-level = 2`, so the debug viewer stays smooth, and
without debug info, so a cold build is faster and `target/` far smaller. Chorong's own crates
keep full debug info. Backtraces through dependency frames show no file or line. To debug
inside a dependency, override it for one build:
`cargo build --config 'profile.dev.package."*".debug=true'`.

## Known pin

`app/Cargo.toml` pins `libc = "=0.2.186"` on Linux: gpui 0.2.2 pulls in `xattr` 0.2.3, which
uses `libc::ENOATTR`, a constant newer libc releases dropped on Linux. Remove the pin once
gpui depends on a newer `xattr`.

## Bundled fonts

`crates/cho-fonts/fonts/` holds IBM Plex Sans and IBM Plex Mono 2.005 (SIL Open Font
License 1.1), unmodified, with the licence text beside them (`OFL-IBM-Plex.txt`). They are
compiled into the viewer as its default fonts (about 1.1 MB): Sans Regular, Medium and
SemiBold; Mono Regular, Italic and Medium. To update them, replace the TTFs and the licence
and keep the list in `crates/cho-fonts/src/lib.rs` in step.

## Vendored JavaScript

`crates/cho-graph/assets/` holds Cytoscape.js 3.30.2 (MIT) and ELK.js 0.9.3 (EPL-2.0),
unmodified, with their full licence texts beside them; no build step or network is needed.
To update one, replace the file, the licence and the pinned version in `html.rs` and the
NOTICE table in [graph.md](graph.md).
