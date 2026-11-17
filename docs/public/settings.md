# Settings

The backend is built (`crates/cho-settings`, `crates/cho-metrics`, `cho settings`), the
viewer gates each optional feature on its setting and measures it, and the settings window
edits them all. A running viewer picks a change up within about a second, whoever made it.

One place to turn Chorong's optional features off, mainly so that a slow machine, a huge
repository or a heavy indexer never makes reading sluggish. Every optional feature can be
switched off, and the settings page says what each one costs, with numbers measured on your
machine.

## Where settings live

- **Settings window:** Cmd-, (Ctrl-, on Linux), or **Settings** in the command palette. It
  opens as its own preferences-style window, not a tab, and does not take over the code
  window. There is one settings window: opening it again brings it forward. Esc or Cmd-W
  closes it. Changes apply at once and are saved at once; nothing needs a restart.
- **File:** `settings.json` in Chorong's config directory (`$XDG_CONFIG_HOME/chorong`, or
  `~/.config/chorong`; `~/Library/Application Support/chorong` on macOS), next to `trusted.json`.
  JSON, as in Zed, with comments allowed. The page and the file are the same settings: edit
  either.
- **Per repository:** a setting can be overridden for one repository (by canonical path),
  shown on the page as "this repository only". Overrides are stored in Chorong's config
  directory, never in the repository.
- Unknown keys are kept and reported, never deleted; a malformed file is reported and the
  last good settings stay in force.

## The settings window

From the top: the **Lightweight** card; **Costing the most right now** (the three features
with the highest total time in the last five minutes, each with its toggle); then one row per
setting from the table below. Each row has the toggle or number field, the description, the
cost hint, a **cost class** chip, the **measured cost** (runs, average, worst; CPU time and
peak memory for background work), a **source badge** and a **this repository only** switch.

Source badges: `default`; `settings.json`; `this repository` (the row is overridden for the
repository of the code window the page was opened from, and is tinted); `Lightweight` (the
value is forced off, and the row says what it would be otherwise).

Syntax errors in `settings.json` or `repos.json` and unknown keys are shown as notices at the
top; a file with a syntax error is not overwritten.

| Key | Action |
| --- | --- |
| Up, Down, Home, End | move between rows |
| Space | flip the toggle on the selected row (on a number, start editing) |
| Enter | edit a number (digits, Enter saves, Esc cancels); on a toggle, flip it |
| R | switch the row between everywhere and this repository only |
| Backspace | reset the row in the scope it is in (repository override first, then `settings.json`) |
| any other letter, or `/` | start searching (or click the search box at the top): rows whose key, description or cost hint contains the text; the box shows how many match, and Esc or its × clears it |
| Esc, Cmd-W | close (Esc first clears a filter) |

R turns "this repository only" on by writing the row's current value as an override for the
repository, and turns it off by removing that override. With no repository open the switch
says so and does nothing. The mouse does the same: click a toggle, a number field or the
scope switch.

## Projects, Cache and Accounts

The window's header has four sections: **Settings** (everything above), **Projects**, **Cache**
and **Accounts**, for the remote repositories described in [Remote sources](sources.md). Click
a section or press Ctrl/Cmd-1 to 4. They add **no setting keys**, and none of them works in the
background: nothing runs until you open a section or press a button, and Lightweight mode does
not change them. All the reading and writing happens off the window's thread, so it stays
responsive.

- **Projects** lists each remembered project (`github:acme/api`) with how it is cloned (HTTPS
  or SSH), the number of worktrees it keeps (1 to 50; click the number, type, Enter), its
  prepare approval (`approved at 3f9c1e2`, `not approved`, or `no recipe`, meaning no recipe
  of the project's own is saved). Nothing prepares by itself: the window's banner or Open
  Remote's box starts it. Each change is written to `projects.json` at once, atomically. **Remove project...** asks first and says what goes: the project entry,
  Chorong's cached clone and worktrees (with their total size) and their indexes. Your own
  clones are never touched.
- **Cache** shows the cache folder (with **Reveal**), the total size, and for each project the
  size of its clone and of each worktree (`pull request 12`, `branch main`), with when it was
  last opened. The sizes are measured when you open the section ("Measuring..." meanwhile) and
  again after a cleanup. **Remove** (one worktree), **Clear** (one project's clone and
  worktrees) and **Clear all** each ask first and name what goes and its size; indexes of what
  is removed go too. A cleared project stays a project and is cloned again when opened.
- **Accounts** has one row per host: github.com, bitbucket.org and each Data Center host of
  your projects. It shows where the token comes from (keychain, or environment and the
  variable), whether Bitbucket Cloud has its account email, and **Check**, which asks the host
  once and shows `valid`, `refused by the host` or `not checked (why)` and the REST quota
  (Data Center hosts are not checked). **Add token...** opens a form with a masked token field
  (and an email field for Bitbucket Cloud); the token is stored in the keychain only and the
  field is cleared at once. **Remove token...** deletes the keychain entry. Without a keychain
  (a Linux machine with no Secret Service, a container) the section says so and names the
  environment variable to set instead, and offers no form. A token is never shown, logged or
  written to a settings file.

## Lightweight mode

One switch at the top of the window (and `chorong::ToggleLightweightMode` in the palette) turns
every optional feature below off at once, leaving a plain, highlighted, read-only viewer:
syntax highlighting, outline, back/forward, the file tree, pickers and search on demand
(highlighting and the outline have no switch and always stay on; folding by default is a
toggle and turns off). Every toggle marked for Lightweight turns off; number settings stay
editable. Turning it off restores each feature's own setting. The status bar shows
`Lightweight` while it is on, and the index status reads `Index: off (Lightweight)`. The index
banner still shows and still starts a build when you click it; nothing starts by itself.

## Optional features

Each row on the page has the toggle, a one-line description, a **cost hint** (what kind of
work it does and when), and **measured cost**: what it has actually cost in this session
(count, average and worst time, or memory), from the instrumentation below. Defaults are on
unless stated.

| Setting | What it does | Cost hint |
| --- | --- | --- |
| `appearance.code_font` | Font for code: the source view, previews and other code text (default IBM Plex Mono) | Negligible: text is drawn either way. A font that is not installed falls back to the bundled IBM Plex Mono. |
| `appearance.ui_font` | Font for the interface: panes, headers, the status bar and menus (default IBM Plex Sans) | Negligible: text is drawn either way. A font that is not installed falls back to the bundled IBM Plex Sans. |
| `blame.enabled` | Toggle Blame (Cmd-K Shift-B): who last changed each line, in a column beside the gutter | Nothing until Toggle Blame is on. Then one `git blame` per file and revision shown, in the background, cached for that commit and text (plus a `git rev-parse` per code pane when the window is activated, to notice a new commit): under 10 ms for a small file, under 100 ms for a 2,000-line file with 100+ commits (measured: 70 to 81 ms in the viewer for tokio's 2,276-line `runtime/builder.rs`, 132 commits); seconds on a very long, deep history. Off, and always in Lightweight mode: Toggle Blame says it is turned off. |
| `changes.auto_refresh` | Refresh the Changes list when the window is activated | **Heavy, in the background:** re-diffs the whole comparison on each window activation: about 15 ms on a small repository, about 2 s on a large one (measured in a debug build). Off, and always in Lightweight mode: the list refreshes only on Compare Against, Show Changes and its Refresh button, and says it may be out of date after the window was activated. |
| `compare.changed_words` | While comparing, a darker block on the words that differ within a changed line | Finds the changed words in the background with the line diff when a file is prepared (measured: about 40 ms in a debug build for a 1,300-line file with a large rewrite), then draws one box per changed word on the visible lines only: under 1 µs a line (debug build), no measurable frame time. Off, and always in Lightweight mode: changed lines keep their tint, without word blocks. |
| `compare.follow_highlight` | In a linked split (Compare Side by Side), briefly highlight the declaration the other column follows to: `off`, `outline` (a still outline, for reduced motion) or `fade` (default fade) | Redraws the followed column for 0.9 s after each follow: about 27 frames while it fades, one after the outline; each costs what a normal redraw of that pane costs. `off`, and always in Lightweight mode: the column moves without a highlight. |
| `compare.line_highlight` | While comparing, tint changed lines and draw a rule where the other side has lines: `off`, `subtle` or `stronger` (default subtle) | Draws a background and at most one rule per visible line, from marks worked out once per file and comparison: under 1 µs a line. Scrolling a 1,300-line file with a large diff (debug build) gave the same frame times off, subtle and stronger: p50 5.3 to 5.5 ms, p95 6.2 to 6.8 ms. `off`, and always in Lightweight mode: the gutter bars and triangles only. |
| `context.follow_hover` | Context pane follows the word under the pointer | A lookup each time the pointer rests on a word (after `context.hover_delay_ms`, default 300). Small per lookup; adds up when moving the pointer a lot. |
| `context.hover_delay_ms` | How long the pointer rests on a word before the context pane follows it (default 300) | A longer delay means fewer lookups while moving the pointer. |
| `context.follow_caret` | Context pane follows the caret | A lookup per caret move. Small. |
| `context.references` | Context pane lists references | Reads a preview line from each referencing file, in the background. Grows with the number of references. |
| `hover.card` | Hover card with type, signature and docs | One index lookup per hover. Small. |
| `hover.peek` | Definition peek in the hover card | Parses the defining file's outline once (cached). The first peek in a large file costs the most. |
| `highlight.exact` | Exact (index) mode for double-click highlights | One index lookup per highlight. Small; off falls back to name matching. |
| `highlight.gutter_marks` | Mark highlighted lines in the gutter (a bar in the group's colour). Also toggled from the Highlights pane's header | Negligible: drawn from the same list as the highlight boxes. Stays on in Lightweight mode. |
| `lens.call_kinds` | Critical Lens colours calls by kind | One index lookup per call on screen when the lens is on. Grows with calls per screen. |
| `index.auto_refresh` | Keep the index fresh in every session: refresh a stale index without asking, as the index banner's **Keep fresh this session** does for one window (default false). Never a first build: every build starts with a click | **Heavy, in the background**, each time the code changes enough to make the index stale: runs the indexer, which runs the project's own code (Rust build scripts and macros). Off, and always in Lightweight mode: a stale index shows the banner's **Refresh**. |
| `index.banner` | Show the index banner (what a build runs, with its buttons) when there is no index or it is out of date (default true). The banner's **Don't show again** turns this off | None: a card drawn while the index is missing or stale. Off: build or refresh from the status bar's Index item, Cmd-K I or the palette; nothing builds by itself either way. Stays as set in Lightweight mode. |
| `index.low_priority` | Run indexers below the viewer's priority (`nice -n 10`; default true) | None while reading. On, a build yields the CPU to the viewer; off, it finishes sooner but can make the viewer stutter. Stays as set in Lightweight mode. |
| `index.node_memory_mb` | Memory limit for the TypeScript indexer's Node process, in MB (default 8,192) | Raises the ceiling only; a large pnpm or Turborepo monorepo can use most of it while indexing (several GB). A `NODE_OPTIONS` you set yourself with `--max-old-space-size` wins. If the indexer still runs out ("JavaScript heap out of memory"), the failure message names this setting. |
| `index.parallel_projects` | How many project indexes build at once (default 1) | More is faster to finish but competes with reading for CPU. |
| `index.typescript_version` | Version of scip-typescript that Chorong installs and runs (default 0.6.0): an exact version, a range (`^0.6.0`, `~0.6.0`) or `latest`. See [semantic.md](semantic.md#the-indexers-chorong-runs) | Nothing until a build. A version Chorong has not installed yet is downloaded once, at the start of the next build (36 MB on disk with its TypeScript; 1.5 to 5 s measured); installed versions stay in Chorong's cache. |
| `index.python_version` | Version of scip-python that Chorong installs and runs (default 0.6.8), as for `index.typescript_version` | Nothing until a build. A version not installed yet is downloaded once, at the start of the next build (73 MB on disk; 4.4 s measured). |
| `index.version_check` | Check the registry for a newer match of a range or `latest` indexer version, every `index.version_check_days` | One `npm view` (a registry request: 0.3 to 0.7 s measured) when a build you started begins and the last check is older than `index.version_check_days`; nothing for an exact version. Off, and always in Lightweight mode: the installed match runs until the setting changes, and the registry is asked only when nothing installed matches. |
| `index.version_check_days` | Days between registry checks for a range or `latest` indexer version (default 7) | Fewer days means more registry requests, each under 1 s at the start of a build. |
| `index.typescript_install` | Your own scip-typescript (its package folder or its `dist/src/main.js`) instead of Chorong's copy; empty uses Chorong's | None: Chorong downloads nothing and runs your copy with `node`. |
| `index.python_install` | Your own scip-python (its package folder or its `index.js`) instead of Chorong's copy; empty uses Chorong's | None: Chorong downloads nothing and runs your copy with `node`. |
| `investigations.watch` | Watch the open tour or call stack file and offer **Updated · Reload** when it changes (the content is never swapped by itself) | While an investigation is open: one file `stat` every 2 s and on window activation (about 0.5 µs each, measured in a debug build on Linux: `cargo test -p chorong-app stamp_cost -- --ignored`), plus one read and parse of that file when it changed. Nothing else is read, fetched or indexed. Off, and always in Lightweight mode: edits to the file are noticed only when you open it again. |
| `tools.node` | Path to `node`, which runs the TypeScript and Python indexers. Empty: found on first use and saved here | When empty, the first build that needs it starts your login shell once to read its `PATH` (0.01 s measured for bash with a plain profile; a profile that loads nvm takes longer; stopped at 5 s) and looks in the known places; then nothing. |
| `tools.npm` | Path to `npm`, which installs Chorong's copies of the indexers. Empty: found on first use (next to `node` first) and saved here | When empty, found with `node` the first time an indexer is installed; then nothing. |
| `tools.uv` | Path to `uv`, which a Python project's prepare recipe (`uv sync --frozen`) runs. Empty: found on first use and saved here | When empty, the first recipe that runs `uv` starts your login shell once (as for `tools.node`); then nothing. |
| `search.max_results` | Cap on project search results (default 5,000) | Lower is lighter on memory for very common terms. |
| `picker.preview` | File preview in Go to file (Right arrow opens it beside the list, Left arrow closes it) and in full-screen search ([user guide](user-guide.md#finding-files)) | Nothing until you open it. While open: one background read of the file under the selection (the usual 2 MB source limit) once the selection rests 150 ms, and syntax colours for the 200 lines shown, measured at about 2 ms in a release build (200 lines of Rust or Python; 13 to 18 ms would be the whole 2,000-line file, which is never coloured). No folds and no index; a binary file is read but not drawn, and an image is drawn from its file (20 MB at most). Off, and always in Lightweight mode: no preview and no reads. |
| `editor.fold_bodies` | Function bodies folded by default | Negligible; a reading preference. |
| `editor.json_folds` | Fold JSON and JSONC objects and arrays ([user guide](user-guide.md#reading)) | One extra parse per JSON file on open, in the background: about 70 ms per 1 MB in a release build (highlighting the same file takes about 200 ms). Files up to 256 KB get their folds before they show; a larger one shows at once and its fold marks appear when the parse ends. Off, and always in Lightweight mode: JSON files have no folds and no extra parse. Applies to files opened afterwards. |
| `editor.minimap` | Minimap at the right edge of a code pane maximised on its own, or of each of two code panes on a focus page that are at least 640 px wide ([user guide](user-guide.md#the-minimap)) | Builds the file's miniature once per file, fold change and comparison (about 3 ms for 10,000 lines in a release build) and places the highlight groups on it once per change of the groups (0.1 ms for 100 occurrences, about 2 ms for 10,000). Each frame the pane draws it paints only the rows in the strip: 0.55 to 0.75 ms per frame on that file, however many highlights there are. Mean frame time while scrolling: 4.2 to 5.2 ms against 3.8 to 4.5 ms with it off. Two code panes on a focus page (1,700 px window): 0.26 ms per frame for both strips, mean frame 5.3 to 5.5 ms against 4.8 to 4.9 ms with it off. Off, and always in Lightweight mode: no strip and no work. |
| `editor.minimap_always` | Show the minimap in every code pane at least 640 px wide, not only a maximised one (default false) | Nothing of its own: widens where `editor.minimap` shows, at its cost per pane. Off by default, since the strip takes 100 px. |
| `editor.overview` | Overview lane: a 10 px strip at the right edge of a code pane that has highlight groups or is comparing, mapping the whole file to its height, with the highlight groups, changed lines, the part on screen and the caret ([user guide](user-guide.md#the-overview-lane)) | Places the marks once per change of the highlights, folds or comparison (0.1 ms for 100 occurrences, about 1 ms for 10,000), then paints at most one mark per run of touching marks each frame: 4 to 115 µs per frame from 0 to 10,000 highlights, with no measurable change in frame time. Off, and always in Lightweight mode: no lane. |
| `scroll.overscroll` | Lists and panes stretch and spring back when a trackpad scroll runs past an end, as in other macOS apps | A little arithmetic per trackpad scroll event in a list, and a redraw each frame for about half a second while a stretched list springs back. Off, and always in Lightweight mode: lists stop dead at their ends. |
| `graph.enabled` | Show graph command | Nothing until used; builds the graph in the background on request. |
| `debug.frame_overlay` | Frame-time overlay | Draws a small overlay every frame. Off by default. |
| `debug.frame_log` | Write one line per frame to the file named by `CHORONG_FRAME_LOG` (does nothing while that is unset) | A formatted line per frame sent to a writer thread, only while `CHORONG_FRAME_LOG` is set. Small; a disk write every 200 ms. |
| `layout.autosave` | Remember the pane layout for each repository | A copy of the layout and a small file write (off the UI thread, debounced) after each layout change. Nothing while the layout is unchanged. |

**Retired keys.** `index.auto_build` and `prepare.auto` are gone: nothing indexes or prepares
until you click. A settings file that still has them keeps them and says they were retired
(`cho settings` lists them with the unknown keys); setting one says why.

New optional features add a row here when they land, with the same three parts. A feature
that does work on every pointer move, every frame or in the background must have a setting.

## Measured cost

So that the hints are evidence rather than guesses, each optional feature records, for the
current session: how many times it ran, its average and worst time, and for background work
its total CPU time and peak memory where the platform reports them. The page shows these
next to each toggle, and sorts a **"Costing the most right now"** summary to the top: the three
features with the highest total time in the last five minutes, each with its toggle. Frames
that missed the 60 Hz or 120 Hz budget are counted too, with the features that ran during
them.

Recording is cheap (a counter and a timer per call) and is always on; it never leaves the
machine.

### What the viewer measures

| Setting | Timed | Counts as |
| --- | --- | --- |
| `changes.auto_refresh` | each automatic recompute of the Changes list, in the background (a refresh you ask for with the button or Show Changes is not counted) | background |
| `compare.changed_words` | building the changed-word blocks of each visible line (finding the words runs with the diff in the background; `CHORONG_SWITCH_LOG` prints its time as `WORDS us=`) | foreground |
| `compare.follow_highlight` | each redraw of the followed column while the highlight shows | foreground |
| `compare.line_highlight` | working out each visible line's tint and deletion rules | foreground |
| `context.follow_hover` | the pointer-move and dwell handling on the UI thread (the index lookup itself runs in the background) | foreground |
| `context.follow_caret` | the caret-driven context lookup start on the UI thread | foreground |
| `context.references` | looking up and previewing the reference list, in the background | background |
| `highlight.exact` | the index lookup for a double-click highlight, in the background | background |
| `lens.call_kinds` | classifying a file's calls, in the background | background |
| `index.auto_refresh` | each automatic indexer run ("Keep fresh") (wall time and this thread's CPU; the indexer subprocess's own CPU and memory are not captured yet). A build you start yourself is not counted | background |
| `investigations.watch` | each look at the open investigation file's stamp (a `stat`, on the UI thread), and reading and parsing it when it changed (in the background) | foreground and background |
| `search.max_results` | each project search, in the background | background |
| `picker.preview` | reading a previewed file, in the background | background |
| `graph.enabled` | building the graph, in the background | background |
| `editor.fold_bodies` | choosing the default folds when a file opens | foreground |
| `editor.json_folds` | finding a JSON file's folds after it shows (files over 256 KB), in the background; a smaller file's folds are found inside its load and not timed separately | background |
| `editor.minimap` | building a file's miniature (once per file, fold change and comparison), painting the strip each frame, and placing highlight groups on it | foreground |
| `editor.overview` | placing the lane's marks (once per change of the highlights, folds or comparison) and painting the lane each frame | foreground |
| `scroll.overscroll` | handling each trackpad scroll event in a list, and each spring-back frame | foreground |
| `debug.frame_overlay` | drawing the overlay | foreground |
| `debug.frame_log` | formatting a frame-log line | foreground |
| `layout.autosave` | handing the layout to the saver thread | foreground |

Every frame is also measured against the 60 Hz and 120 Hz budgets, from the start of the
shell's render to the end of paint.

`CHORONG_METRICS_DUMP=FILE` makes the viewer write the report to `FILE` once a second (one line per
feature that has run: calls, total, average and worst microseconds, then the frame budget
line). It is for measuring; the settings page will show the same numbers.

### Gating

Turning a feature off stops its work, not only its output: the viewer does not look anything up,
start a timer, read previews or start an indexer for it. A setting changed in the file applies
at once, with these exceptions: `editor.fold_bodies` and `editor.json_folds` apply to files opened afterwards, and a
`hover.card`, `hover.peek` or `index.parallel_projects` value has no effect yet because the
feature is not built (indexes build one project at a time).

## Keys

| Key | Action |
| --- | --- |
| Cmd-, | open Settings (`chorong::OpenSettings`) |

Lightweight mode has no default key; it is in the command palette and the Cmd-K popup can
be given one through the keymap.

## What is built

**In the viewer** (`app/src/features.rs`): one `Features` owns the settings store and the
metrics; each call site asks `on(Feature)` and holds a timer for the same key. A test
checks that every setting is either gated there or listed with the reason it is not.

**No GPUI dependency:**

- `cho-settings`: the table of settings above as data (`defs()`: key, default, description,
  cost hint, cost class, whether Lightweight turns it off) and as a typed `Settings` struct;
  resolution `defaults < global < repository override < Lightweight` with the source of every
  value (`Effective`); `SettingsStore` to load, save, switch repository and detect external
  edits.
- `cho-metrics`: per-feature counters keyed by the same setting keys, five-minute window,
  frame-budget tracking, "costing the most right now", CPU time and peak memory (Linux only so
  far; other platforms report "unavailable").
- `cho settings` ([cli.md](cli.md#cho-settings)).

### Files

All in Chorong's config directory, never in a repository.

| File | Holds |
| --- | --- |
| `settings.json` | global settings, JSON with comments, flat dotted keys: `"hover.card": false` |
| `repos.json` | per-repository overrides: `{"repos": {"<canonical path>": {"hover.card": false}}}` |
| `trusted.json` | folders you trust to run an index build (each build still starts with your click; not part of this crate yet) |

`CHORONG_CONFIG_DIR` replaces the whole `chorong` directory (tests and screenshots).

Resolution, highest first: **Lightweight** (for settings marked as such, and only while the
`lightweight` setting is on) > **repo** > **global** > **default**. `lightweight` is itself a
setting, so it can be switched on for one repository. Every effective value carries its
source, and Lightweight also keeps the value it covered ("would be true, from repo").

### Reading and saving

- Comments (`//`, `/* */`) and trailing commas are accepted. Nested objects
  (`"context": {"follow_hover": false}`) are not: they are reported as an unknown key with a
  hint to use dotted keys.
- **Unknown keys** are reported and left in the file. **Wrong type or out of range** is
  reported and that key falls back to the next layer down. **A syntax error** is reported
  with line and column and the last good settings stay in force; saving then refuses rather
  than overwrite the broken file.
- **Saving edits the text in place:** only the changed value's bytes change. Comments, key
  order, blank lines, indentation and unknown keys stay. A new key is appended to its object
  in the file's own indentation. Removing a key removes its line, its comma and a `//` comment
  trailing it; a comment on the line *above* a removed key stays. Writes go through a
  temporary file and a rename.
- **Change notification:** every mutating call and `reload_if_changed()` returns a `ChangeSet`
  (the keys whose effective value changed, and whether diagnostics changed);
  `store.version()` increases by one per change that moved a value.
- **External edits:** call `reload_if_changed()` on a debounced timer (about a second). It
  `stat`s two files and returns `None` unless one changed (modification time and length). No
  thread is started and no watcher dependency is used.

### Measuring cost

Register once with the setting keys, then time each call by `FeatureId`:

```rust
let metrics = Metrics::new(&[("context.follow_hover", Kind::Foreground), ("index.auto_refresh", Kind::Background)]);
let id = metrics.id("context.follow_hover").unwrap();   // once, at startup
let _t = metrics.start(id);                              // records on drop
```

Per call: two clock reads and a handful of relaxed atomic updates; no allocation, no lock
(about 105 ns on the machine this was measured on, 35 ns of it the atomics; see
`crates/cho-metrics/examples/bench.rs`). The window is sixty five-second buckets, so
"last five minutes" is accurate to five seconds, and a sample racing with a bucket reset can be
lost (totals never are). Foreground features that ran during a frame slower than 8.3 ms or
16.7 ms are counted against it; background features never are. Background CPU time and peak
memory come from `/proc` on Linux: `background_span` for in-process work (thread CPU time),
`record_background_usage` for a subprocess such as the indexer (sample it with
`usage::process_by_pid`).
