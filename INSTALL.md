# Installing Chorong on a Mac

From this folder, these steps build the viewer app and the `cho` command-line tool, install
both inside `Chorong.app` in `/Applications`, and put them on your PATH. They need an Apple
Silicon or Intel Mac, and about 15 minutes for the first build. For Linux and other details
see [docs/public/building.md](docs/public/building.md).

There are two programs:

- `chorong-app`: the viewer window. It lives inside `Chorong.app`.
- `cho`: the command-line tool. `cho .` opens a folder in the viewer, and `cho search`,
  `cho outline` and the other commands work without a window.

## Before you start (once)

1. **Rust**, from [rustup.rs](https://rustup.rs):

   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **Xcode** from the App Store (the command-line tools alone are not enough: the viewer's
   GPU shaders need Xcode's Metal compiler). Then select it and add its Metal Toolchain:

   ```bash
   sudo xcode-select -s /Applications/Xcode.app
   xcodebuild -downloadComponent MetalToolchain
   ```

3. **Node.js**, if you want TypeScript or Python projects indexed (definitions, references,
   the Critical Lens's call kinds). Chorong installs its own copies of those indexers with `npm`
   and runs them with `node`, which come with Node.js.

## Build and install

Run these in Terminal, in this folder:

1. Build the app. This builds the viewer and the `cho` command, puts both inside `Chorong.app`,
   and signs it for this Mac (no developer account needed):

   ```bash
   scripts/bundle-macos.sh
   ```

2. Move the app to `/Applications`:

   ```bash
   mv target/release/Chorong.app /Applications/
   ```

3. Put the app's program folder on your PATH (the script prints this line too), then open a new
   Terminal tab:

   ```bash
   echo 'export PATH="/Applications/Chorong.app/Contents/MacOS:$PATH"' >> ~/.zshrc
   ```

## Use it

- `cho .` or `cho ~/code/some-project` opens a folder in the viewer, with Chorong's icon.
- `cho search …`, `cho outline …` and the rest: `cho --help` lists them.

Chorong can also start from Finder or the Dock. The first TypeScript or Python build looks for
`node` and `npm` in your login shell's PATH and the usual install places (Homebrew, nvm, volta,
fnm) and saves them as the `tools.node` and `tools.npm` settings. If it finds none, the error
lists where it looked; set the path with `cho settings set tools.node /path/to/node`.

## If something goes wrong

- **`unable to find utility "metal"`** or `bundle-macos: no Metal compiler`: step 2 of
  "Before you start" is missing; `scripts/bundle-macos.sh` prints the two commands.
- **"Chorong can't be opened" or "is damaged"** after copying `Chorong.app` from another Mac:
  right-click it and choose **Open**, or allow it under **System Settings → Privacy &
  Security**, or run `xattr -dr com.apple.quarantine /Applications/Chorong.app`.
- **`cho: command not found`**: open a new Terminal tab after step 3, or check that
  `~/.zshrc` has the PATH line.
