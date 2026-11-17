# Changelog

Release notes for published versions. Newest first. New entries are written as files in
`changelog.d/` and compiled into this file at release (`scripts/changelog.py --release VERSION`).

## Unreleased

### Settings
- Settings backend: `odin-settings` (every optional feature as a typed setting with a cost hint;
  `settings.json` with comments; per-repository overrides kept in Odin's config directory;
  Lightweight mode; change notification) and `odin-metrics` (always-on per-feature timings,
  five-minute window, frame budgets). The settings page itself follows.
- `odin settings`, `odin settings get|set|unset KEY [VALUE] [--repo DIR]`: show and change
  settings with the source of every value.

### Viewer
- The viewer's panes now sit in a layout: pane headers (minimise, maximise, close, drag to move),
  left, right and bottom docks, dock toggles in the status bar, and keyboard Layout mode (`w`).
  The layout is saved per repository (off the UI thread) and restored. Cmd-K O now folds the
  Outline and Cmd-K B toggles the left dock. Cmd-K P still toggles the viewer's pin. The first
  editor pane shows the open file; split editors follow. `debug.frame_overlay` shows frame times.
- `odin PATH` is shorthand for `odin open PATH` (`odin .`, like `zed .`). On Linux a second
  `odin-app PATH` hands the path to the running viewer instead of starting another one.
- Export for sharing (`odin graph --share`, the viewer's graph export and **Copy odin:// Link for
  Sharing**) refuses a repository that would be `local:/absolute/path` (outside the home folder,
  no remote) and says to add a remote or pass `--repo`. Your own links are unchanged.
- On Linux the viewer picks an installed UI font (Inter, Cantarell, Ubuntu, Noto Sans, DejaVu Sans,
  Liberation Sans) instead of GPUI's `.SystemUIFont`, which isn't installed on typical systems and
  makes every text run repeat a failed lookup. macOS is unchanged. The shared picker is the
  `odin-fonts` crate.
- Read-only GPU viewer (GPUI) for files and folders, with Zed theme support and a warm Odin
  Dark / Odin Light pair.
- Fuzzy file picker, outline panel, function bodies folded by default, exact back/forward,
  background loading, keyboard line selection; file lists respect `.gitignore`.
- The reading loop: double-click highlighting (same name in the enclosing declaration, or exact
  symbol with an index), a context pane that follows the caret and hovered identifier, Critical
  Lens (call names stand out; calls classified as repo, external or unknown with an index), and
  a status bar that says what's known.
- Repository search (Ctrl/Cmd-Shift-F).
- Command palette (Ctrl/Cmd-Shift-P) and one bundled keymap (`app/keymap/default.json`, Zed
  format): Cmd-K chords with a popup, single-key reading loop (`h`, `n`, `N`, `d`, `r`, `[`, `]`),
  `?` cheat sheet. The outline panel moved to Cmd-K O and the Critical Lens to Cmd-K L.
- Highlight groups are drawn as rounded markers behind the text, with the declaration outlined
  where it is known.
- Build the index from the viewer (status bar, context pane, palette, Cmd-K I): background build
  with progress, asks to trust the folder once, loads in place, refreshes stale indexes in
  trusted folders, shows the error when it fails. Never installs packages.
- Open the current line in Zed, VS Code, Cursor, Sublime, Xcode or JetBrains IDEs.
- The context pane keeps the hovered symbol while the pointer moves into the pane, so its
  references can be clicked; a Pin (Cmd-K P) keeps a symbol until unpinned.
- Shift-F12 / `r` move the keyboard into the pane's reference list (Up/Down, Enter, Esc).
- Critical Lens without an index is the plain lens (no "unknown" marks); the status bar says
  call kinds need an index.
- The Cmd-K popup holds a chord open after GPUI's one-second timeout, until the next key.
- Show graph (Cmd-K G, palette, context pane, sidebar right-click): a popover for start,
  direction, depth, edge kinds and sharing; the page is built in the background, written to
  Odin's cache and opened in the browser.
- `odin://` links: remote links open a known clone of the repository (or offer the web page);
  Copy odin:// Link (Cmd-K C) and Copy odin:// Link for Sharing.
- A banner for links made at another commit: View at that commit (read from git,
  read-only), Open worktree, Rebuild graph, and the `git switch` command to copy.
- Registered as the `odin://` handler (Linux `.desktop` file and install script; macOS
  `.app` bundle metadata); on Linux a second launch hands its link to the running viewer.

### Graph
- `odin graph`: neighbourhood, file, package, outbound closure and local-branch blast radius
  from the semantic index, as JSON, DOT or a self-contained offline HTML page (header with
  worktree, branch, commit and dirty state; `odin://` links on every node).
- `odin://` links accept `branch` and `dirty=1`; `odin link build` has `--branch` and `--dirty`.
- Links name the repository by its remote (`github:`, `bitbucket:`, `bitbucket-dc:`), or
  `local:~/…` without one; never the absolute path under home. `odin graph --share` drops
  branch, dirty state and worktree name. Graph links carry `from=graph`.

### Semantics
- `odin hover --peek` and `Semantic::peek_definition` now answer for fields, properties, parameters
  and locals: the enclosing declaration with the member's line(s) marked (`kind: "enclosing"`,
  `marked_range`). `no-declaration` is left for definitions that nothing in the outline contains.
- SCIP indexes for Rust, TypeScript and Python: definition, references, symbol search. Never
  guesses by name; stale indexes are labelled.
- `odin hover FILE LINE:COL`: the indexer's signature (inferred locals included), docs,
  definition and the index's commit and status. Says so when the index has no signature.
- Hover has canonical names: `name` (the index's display name, else the symbol ID's last
  descriptor, else the source text for locals) and `local_text` (what the occurrence says, e.g.
  `plus` for `import { add as plus }`).
- Definition peek: `odin hover --peek` (and `Semantic::hover_at_with_peek`) gives each
  definition's full declaration range from the Tree-sitter outline, or says why there is none.
  Computed on request only, with a per-file outline cache.

### CLI (`odin`)
- Headless commands with JSON output for people and agents: outline, find, show, highlight,
  search, index, definition, references, symbols, investigations, links, editors, themes.
- Self-describing formats for agents: `odin formats`, `odin schema`, `odin skill`.

### Investigations
- A JSON format for call stacks, tours, diagrams and PR overlays, checked against the code
  (`verified`, `claimed`, `broken`).
