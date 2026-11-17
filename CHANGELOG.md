# Changelog

Release notes for published versions. Newest first. New entries are written as files in
`changelog.d/` and compiled into this file at release (`scripts/changelog.py --release VERSION`).

## Unreleased

## 0.1.1 (2026-11-17)


### CLI (`cho`)
- Each crate links to its source repository, so links in the crate READMEs work on crates.io.

## 0.1.0 (2026-11-17)

### Settings
- `compare.line_highlight` (`off`, `subtle`, `stronger`; default `subtle`) and
  `compare.changed_words` (default on) control the change highlights in the code; both are off in
  Lightweight mode. Settings can now be a choice of a few words, shown as a segmented control.
- The viewer now obeys its settings: every optional per-pointer, per-frame and background
  feature (context follow, references, exact highlights, call-kind colours, automatic indexing,
  graph, default folds, frame overlay and log, layout saving) stops its work when switched off
  or in Lightweight mode, applies changes without restart, and is timed under its setting key
  (`CHORONG_METRICS_DUMP=FILE` writes the report). New settings: `debug.frame_log`, `layout.autosave`.
- `highlight.gutter_marks` (on by default) shows or hides the highlight marks in the gutter; the
  Highlights pane's header has a switch for it.
- New `index.banner` setting (on by default) shows or hides the index banner. The banner's
  **Don't show again** turns it off for every window; the status bar's Index item, Cmd-K I and
  the palette still build and refresh.
- New setting `index.node_memory_mb` (default 8192): the memory limit for the TypeScript
  indexer's Node process. A `NODE_OPTIONS` with its own `--max-old-space-size` still wins, and a
  "JavaScript heap out of memory" failure now names the setting.
- New settings: `index.typescript_version` and `index.python_version` (an exact version, `^`/`~`
  range or `latest`; a newer indexer needs no new Chorong), `index.version_check` and
  `index.version_check_days` (how often a range or `latest` is checked, weekly by default; off in
  Lightweight mode), `index.typescript_install` and `index.python_install` (your own install;
  Chorong then downloads nothing), and `tools.node`, `tools.npm`, `tools.uv` (filled the first time
  Chorong finds them).
- `investigations.watch` (on by default, off in Lightweight mode): the open tour or call stack
  file is watched, and **Updated · Reload** appears when a newer version parses.
- Pane headers and tabs switch to a new interface font as soon as `appearance.ui_font` changes,
  like the rest of the window (they used to wait for the next start).
- `index.auto_build` and `prepare.auto` are retired: a settings file that still has them keeps
  them and says they were retired. `index.auto_refresh` now means "keep the index fresh in every
  session" and is off by default (and in Lightweight mode). New: `index.low_priority` (on by
  default) runs indexers below the viewer's priority.
- Open Remote's **Prepare and index** box is unchecked by default and never remembered; a
  project's auto-prepare is ignored (dropped from `projects.json` on the next save) and the
  Settings Projects switch is gone.
- Removing a setting whose trailing comma sits on a later line or after a comment no longer
  leaves the settings file unreadable.
- The Settings window has a search box: click it or just type to find settings by key,
  description or cost hint, with a count of matches and a button to clear it.
- Settings window (Cmd-, / Ctrl-,, or **Settings** in the command palette): its own window with
  a Lightweight card, "Costing the most right now", and a row per setting with its cost hint,
  measured cost, source badge and a "this repository only" switch. Keyboard first (arrows,
  Space, Enter, R, Backspace, type to filter). `chorong::ToggleLightweightMode` and a status-bar
  `Lightweight` indicator.
- Every text field in the settings window (the search box, a number being typed, a project's
  worktree cap, the token and email of a new account) takes Cmd-A, Cmd-C, Cmd-V,
  Cmd-Backspace and Option-Backspace like the other fields. A token is never copied out.
- macOS: **Settings…** (Cmd-,) in the Chorong menu.
- Settings backend: `cho-settings` (every optional feature as a typed setting with a cost hint;
  `settings.json` with comments; per-repository overrides kept in Chorong's config directory;
  Lightweight mode; change notification) and `cho-metrics` (always-on per-feature timings,
  five-minute window, frame budgets). The settings page itself follows.
- `cho settings`, `cho settings get|set|unset KEY [VALUE] [--repo DIR]`: show and change
  settings with the source of every value.

### Viewer
- Building a focus page with files: Tab in the tray picker (or `f` then Tab) opens a file
  picker that adds each file in a new code pane on the page, Enter to the right and Shift-Enter
  below, and stays open; Backspace on an empty query goes back to the tray. New Focus Page with
  an empty tray opens it straight away.
- Cmd-K A (Add to Page from Tray or Files…, formerly Bring Pane from Tray…) adds to the
  current focus page: the tray picker, with Tab for files. With an empty tray it opens the file
  picker instead of saying the tray is empty.
- Times and sizes read the same everywhere: "1 day ago" (not "1 days ago") in the remote header
  band and `cho` output, "5 min ago" in the settings window as in the rest of Chorong, and sizes
  in binary units (KB = 1024 bytes) in both `cho cache` and the settings window's Cache page.
  The settings page's peak memory no longer shows "0 KB" for less than a kilobyte.
- Toggle Blame (Cmd-K Shift-B) shows who last changed each line beside the gutter: the author's
  initials and the commit's age once per run of lines, shaded by age, with the commit's hash,
  summary, author and date on hover. Lines edited since the last commit say `uncommitted`; a
  pane at another revision is blamed from it. Setting: `blame.enabled`; off in Lightweight mode.
- `scripts/bundle-macos.sh` puts the `cho` command inside `Chorong.app` next to the viewer and signs
  the bundle, so installing is: run it, move `Chorong.app` to `/Applications`, add one PATH line.
- The status bar's caret column no longer crashes when the caret sits inside a multi-byte
  character or past the end of the text (for example after the file shrank).
- The selected line inside a tint (a tour step, a chunk, a comparison's changes) keeps the tint
  and is marked by an accent bar at the code's edge, instead of a grey fill that cut through it.
- While comparing, changed lines get a faint tint in the theme's diff colours (green on After, red
  on Before), and a thin rule crosses the code where the other side has lines this side lacks.
  Hover the rule for how many lines; click it to see them on the other side.
- Changed words: within a lightly edited line, the words that differ get a slightly darker block.
  Lines are paired only when at least half their words match; heavily rewritten lines keep the
  tint alone, and whitespace-only changes say so on hover.
- Uncommitted work is marked by one yellow bracket per run of lines in the gutter, yellow line
  numbers, and a yellow tick at an uncommitted deletion, on Before as well as After. The pane
  header counts it ("3 uncommitted"); click it or press `u` / Shift-U to step through, and
  Highlight Uncommitted (Cmd-K U) tints those lines yellow.
- The Chorong themes define `created`, `modified` and `deleted`; the change marks use them (a true
  yellow for uncommitted work), falling back to `success`, `warning` and `error` in other themes.
- The Changes pane opens by itself when a comparison starts or a pull request opens. Files start
  collapsed, each with a status letter (`A`, `M`, `D`, `R`); Right / Left or the arrow expands
  and collapses one, and **Expand All Changes** / **Collapse All Changes** do all of them.
- Type in the Changes list to filter it: a fuzzy match on file paths and declaration names. A file
  stays when a declaration in it matches, and shows the matching declarations.
- Search has a **Changed** option while comparing: it searches only the files the comparison
  changed.
- The key cheat sheet (`?`) fits the window: three columns, long titles wrap in their column, and
  the groups scroll under the heading instead of running off the bottom and the side.
- The pane menu says **Close pane** instead of **Close**, so it is not mistaken for **Close
  investigation** in the investigation panes' menus.
- The pane that shows a file is now called a code pane (it was "editor pane"; Chorong never
  edits), and the area that holds them the code area, in menus, the pane list, messages and
  the guide. Keymap action names are unchanged.
- Compare Against… (palette) compares the file you are reading with where your branch left another
  branch, tag or commit. Switch Before/After (`b`) flips the pane between the working-folder file
  and the merge-base version, keeping your place by declaration; gutter marks show what changed,
  the status bar says which side you are on, and Stop Comparing returns to the working folder.
- In the compare view, the Before side opens from git even when the working-folder file can't be
  read (for example, it has grown past the size limit). After still says why it can't be read.
- The Changes list: Show Changes opens a pane listing the declarations a comparison with a branch touched, grouped by file, each marked added, changed or removed, with `uncommitted` tags and a count per file. Click a row or press Enter to open the file at that declaration on the right side (Before for a removed one). Ambiguous names list every candidate. In a linked split, the other column scrolls only when the declaration is out of view.
- The Changes pane has a Refresh button in its header. The `changes.auto_refresh` setting (on by default, off in Lightweight mode) refreshes it when the window is activated; when it is off the pane says "May be out of date" instead.
</content>
</invoke>
- Start comparing from more places: **Compare…** in the status bar of a git folder, Cmd-K D
  (Compare Against…), and the Changes pane's menu has Expand all / Collapse all.
- Compare Side by Side splits the pane into Before (left) and After (right), linked by declaration:
  moving the caret into another function or type in one column takes the other to the same
  declaration. Link or Unlink Split (Shift-B) toggles the following; closing a column, opening
  another file in it, or switching its side ends the pair. The status bar shows linked or unlinked.
- While comparing, After's gutter tells uncommitted work from committed changes (outlined marks
  for uncommitted, filled for committed), the status bar counts the files with uncommitted
  changes, and Open in editor on Before opens the working copy at the matching line.
- JSON (`.json`, `.jsonc`), TOML (`.toml`, `Cargo.lock`), XML (`.xml`, `.plist`, `.svg`, `.xaml`
  and other XML formats) and Markdown (`.md`, `.markdown`) files are now syntax-highlighted:
  keys, strings, numbers, tags, headings, emphasis, links, code and comments are coloured.
  They are highlight-only, with no outline or call marks; Markdown shows its source,
  not a rendered preview, and colours fenced code blocks by their language (```` ```rust ````,
  ```` ```json ````, …).
- The context pane's definition and reference previews are syntax-coloured like the code view,
  with the symbol itself in its kind colour. The definition sits in a card with its line number
  at the right, and the "you are in" / "at the caret" labels are fainter.
- Chorong Dark and Chorong Light: clearer status colours (index current, stale, failed), a darker
  muted text and accent in Light, opaque count pills, a visible active indent guide, and the
  outline's current declaration is filled.
- Opening a file no longer recompiles the highlighting queries each time.
- The Context pane keeps its cards and the Symbol row within the pane's width: a long name is
  clipped instead of pushing the right edge and the Show graph button out of sight.
- The `Index: failed` popup has a **Copy error** button, and the prepare-failure notice a **Copy
  output** button.
- Opening a file with very deep nesting (tens of thousands of brackets, or a very long
  `a + b + …` or `.call()` chain, as in generated code) no longer crashes Chorong or takes minutes:
  past 2,000 levels the file is shown without syntax colours, and declarations inside more than
  64 nested scopes are left out of the outline.
- Pressing Shift twice quickly opens the command palette, as Cmd-Shift-P does.
- Esc closes the index banner (as **Not now**, shown on the button) while the code has the
  keyboard, before anything else; it used to skip it and restore the focus layout behind it.
  Enter keeps its meaning in the code: no key starts a build from the banner, so indexing never
  starts by accident (a click, Cmd-K I or the status bar does).
- The focus bar's "Restore layout" button and the pane menu's "Restore size" (in a focus layout)
  are now called "Exit focus", which is what they do.
- Adding files to a focus page shows a **Files** chip before the query, so the mode is visible.
  Backspace on an empty query or a click on the chip goes back to the tray, as Backspace did.
- Find in File (Cmd-F, Ctrl-F off macOS) opens a find bar at the top of the code pane: matches in
  the open file as you type, a "3 of 12" count, **Aa** (Alt-C) and **.\*** (Alt-R) toggles, Enter
  and Shift-Enter to the next and previous match (wrapping), Escape to close. The matches are a
  highlight group, so the gutter, overview lane, minimap and `n` / `N` show them.
- Measured in the viewer (release build, Linux under Xvfb, default settings, a 2 MB Rust file of
  53,518 lines, two runs, `CHORONG_FRAME_LOG`): frames while typing in the find bar take 5.8 to
  6.4 ms median, 10.7 to 11.8 ms at the 95th percentile and 15.3 ms at worst, against 3.9 to
  4.1 ms median (5.2 to 8.0 ms p95) for frames moving the caret and about 4.5 ms for idle pointer
  frames. Queries matching 10,000+ times: 5.5 to 5.7 ms median. Enter and Shift-Enter: about
  5 ms median. Matching itself runs before the frame, on each keystroke: about 1 ms median,
  1.7 ms p95 and 2.5 ms at worst for the whole file. Everything stays inside the 60 Hz budget.
- In a focus layout, Tab and Shift-Tab in Layout mode flip the focused spot through the files in
  the tray, and the tray's chips get jump labels that bring a pane up the same way.
- In a focus layout, Close (the pane menu or a tab's ×) sends the pane to the tray instead of
  closing it; New Code Pane in the command palette opens an empty code pane, on the current
  focus page when one is open.
- Focus layouts in the viewer: a new focus page (+, Cmd-K Shift-N) starts with the focused pane,
  Cmd-K R resets a tour step's page to the panes the step declares, and the palette lists
  Maximise or Restore Pane, New/Next/Previous Focus Page, Reopen Focus Layout… and Undo/Redo
  Layout Change. Focus history is kept per repository across restarts.
- The fold chevron no longer has its own gutter column, so the gutter is narrower. An opened fold
  shows ▾ in place of its line number; a folded one shows ▸ there when you point at it.
- Chorong ships with IBM Plex Sans and IBM Plex Mono and uses them by default, so the viewer looks
  the same on every machine. New settings `appearance.ui_font` and `appearance.code_font` pick
  any installed font; the settings page lists them with a sample of the chosen one.
- The context pane builds only the rows on screen, so symbols with hundreds of references
  scroll smoothly, and its section headers no longer collapse when the list is long.
  Reference previews are syntax-coloured as they scroll into view.
- The Context pane's Pin and ↓ buttons moved into its header, so they no longer cover the
  first lines of the pane.
- The window no longer pauses while git or a folder walk runs: Compare Against… opens at once
  and lists branches and tags as they are read, a typed commit is looked up after a short pause
  in typing, and Build Index and "Keep fresh" check the folder and its trust in the background.
- Reading git state (branch, dirty flag, index freshness) no longer refreshes the repository's
  index: Chorong's `git status` calls run with `GIT_OPTIONAL_LOCKS=0`, so `.git/index` is never
  rewritten and `index.lock` is never taken.
- Opening a folder no longer runs programs named in its git config. A folder unpacked from an
  archive could set `core.fsmonitor`, a filter or diff driver, or a hook, and Chorong's read-only
  `git status` would run it before any trust prompt. Chorong now switches those off for every git
  command it runs. Submodules no longer count towards the "dirty" state.
- On Linux without `XDG_RUNTIME_DIR`, the viewer refuses a link socket folder
  (`/tmp/chorong-UID`) that another user owns or can enter.
- The commands that move the keyboard to another pane are now called Go to Pane… and Go to Pane
  Left/Right/Up/Down, so "focus" in the palette means only the focus layout.
- Highlights: an index answer with overlapping occurrences no longer leaves lines of a
  highlighted symbol without their gutter mark or tint; overlapping ranges are merged.
- A Highlights pane lists the active code pane's highlight groups as chips: the caret's place among
  the occurrences, next and previous, remove, and Clear all. It opens by itself at the third
  highlight, hides when the last one goes, and Cmd-K Shift-H toggles it. It sits in the bottom
  dock, one row high, which can now be as low as 64 px.
- The Critical Lens fades highlighted names less (65% instead of 35%), so they stand out.
- Clear Highlights in the command palette removes every highlight; Esc (now Remove Newest Highlight)
  still removes the newest and, pressed again, the rest.
- Highlighted lines get a bar in the group's colour at the gutter's outer edge, and a fold that
  hides a highlight shows the bar on its row.
- Code panes have Back and Forward buttons in their header, and a mouse's back and forward
  buttons (or a three-finger swipe) go back and forward too.
- A running index build now shows where it is and for how long: `Index: stale · indexing
  packages/core (14 of 38) · 2m 10s`. Elapsed time is always shown, even when the indexer prints
  no steps. A large TypeScript workspace gets a one-line note up front: it may take minutes and GBs
  of memory.
- Index builds can be cancelled: **Cancel Indexing** in the command palette, or click the
  status-bar item (or press Cmd-K I again) and confirm. The indexer and its child processes are
  stopped, and the previous index stays in place, shown as `Index: stale · cancelled`.
- The status bar's index control offers `show error` only when the failed build's error text was
  saved. When the index cache can't be read and there is no error text, it reads
  `Index: failed · build`, and a click rebuilds as the label says.
- Java support (Java 8; newer syntax such as records and text blocks parses too): `.java` files
  are highlighted and outlined in the viewer and the CLI. The outline lists classes,
  interfaces, enums and their constants, annotation types, methods, constructors and fields
  (a new `field` kind; `static final` and interface fields are `constant`). Methods of an
  anonymous class are scoped `Outer.new Comparator.compare`.
- JSON and JSONC files fold: every object and array over three lines, from the syntax tree, with
  the usual fold marks, Space / Enter / Left / Right on a line, and Fold All / Unfold All (Cmd-K 0,
  Cmd-K J) to collapse or expand the whole file. JSON files open unfolded. The fold pass is one
  extra parse per JSON file on open, in the background: about 70 ms for a 1 MB file in a release
  build (highlighting the same file takes about 200 ms). Files up to 256 KB fold before they
  show; a larger one shows at once and its fold marks appear when the parse ends (a Fold All
  pressed meanwhile runs then). New setting `editor.json_folds` (default on, off in Lightweight
  mode).
- After a jump (a reference, a definition, a link, Back), the identifier the caret landed on is
  outlined in the accent colour until the caret moves, so you can see exactly where you went.
- Dialog buttons show their key inside the button (`Show graph  Enter`, `Close  Esc`) instead of
  beside it; the graph dialog's "close" is now a Close button.
- Dropping a whole tab group on a focus page chip, and Move pane to next focus page, are one
  undo step each: one Undo puts everything back (it used to take one Undo per tab).
- Move pane to next focus page no longer reports "that would change nothing" after it moved the
  pane to a new page.
- Closing a focus page (or resetting a tour page) forgets the editor read on it, as sending it
  to the tray does, so Restore keeps the normal layout's own active tab.
- The pane menu, the Reopen focus layout list and the pane pickers are never open together:
  opening one closes the others.
- A layout that cannot be saved (a full disk, a read-only state folder) says so once in the
  status bar instead of failing silently.
- The pane pickers (Go to pane…, Move pane to…) edit their query like the file picker: Cmd-A,
  paste, Cmd-Backspace and word delete.
- In Layout mode, holding Shift (or Alt) lights up where each arrow would move the pane (or its
  group), with the arrow marked on each place.
- Critical Lens: a highlighted external call keeps its 70% brightness instead of dropping to the
  65% given to highlighted names.
- Chorong.app has an app icon: the lantern. `scripts/bundle-macos.sh --icon etched-m` builds it with
  the alternative etched M instead.
- macOS: a menu bar with Quit (Cmd-Q) and a Window menu (Minimize, Cmd-M, and Zoom). The Window
  menu and the Dock icon's menu list the open Chorong windows by name.
- A minimap in a code pane maximised on its own, and in each of two code panes side by side on a
  focus page when both are wide: a strip at the right edge drawing the whole file in miniature in
  its syntax colours, with a frame over the part on screen. Click to centre the code there, drag
  to scroll; changes and highlight groups show on it. `editor.minimap` turns it off (and
  Lightweight mode does); `editor.minimap_always` shows it in every wide code pane.
- Clicking the minimise icon on a folded pane header now expands the pane again, as documented.
  The click was also reaching the header's own expand handler, which folded it straight back.
  The maximise and menu buttons no longer trigger it either.
- Scrolling over Go to file's preview (or any dialog) no longer stretches or scrolls the code
  pane behind it.
- New Focus Page works from the normal layout too (it starts a focus layout with the focused
  pane), and opens the tray picker so the page can be filled pane by pane.
- The viewer and the Changes list agree on which files are binary (a NUL byte in the first
  8,000 bytes, as git decides): a file whose first NUL comes a little later now opens as text.
- Cmd-Enter in the file picker opens the file in a new code pane beside the current one (on the
  current page in a focus layout).
- Cmd-O (Open File or Folder…) opens a file or folder with the system's Open dialog: a file in
  this window's folder opens here, anything else in its own window.
- A `chorong://` link that names a folder you have never opened in Chorong now asks before opening
  it, showing the folder and the link. Any web page or program can send such a link; folders you
  have opened before, and links that name a repository by its remote, open straight away.
- **Open Remote…** opens a branch, pull request or commit of a GitHub or Bitbucket repository
  from the viewer: pick a project, type a branch, `#123`, a commit or a URL, and a hint says what
  it will open before you press Enter. The open runs in the background in a new window with the
  comparison started; **Compare against** can name another branch or commit. **Open Recent…**
  lists opened folders, and the viewer and `cho open` now share one recent-folders list.
- Every scrollable area stretches and springs back at its ends, as the panes already did: Go
  to file's preview and list, the Tour and Call stack lists, the cheat sheet, the Highlights
  pane, the graph, the comparison rows, the step banner's tour rail, prepare's output, and the
  settings window. Off with `scroll.overscroll`, and always in Lightweight mode.
- Trackpad scrolling past the end of the source or a side-pane list (Files, Outline, Search,
  Changes, Context) now stretches and springs back instead of stopping dead. A fast flick into
  an end bounces only a little. Setting: `scroll.overscroll`; off in Lightweight mode.
- An overview lane at a code pane's right edge while it has highlight groups or shows a
  comparison: a thin strip mapping the whole file to its height, showing where each group's
  occurrences and the changed lines are, the part on screen and the caret. Click it to go there.
  `editor.overview` turns it off (and Lightweight mode does).
- The command palette (Cmd-Shift-P) now finds the layout commands too: Close, Minimise and Split
  Pane, Send to Tray, Move Pane to a dock, the dock toggles, Layout Mode, Focus Pane Left/Right/Up/
  Down and Toggle Frame Stats, each with its key.
- The command palette is roomier: a taller input, 30 px rows inset from the edges with rounded
  highlights, a slightly wider window, and each key of a binding in its own cap (⇧ ⌘ P).
- In the command palette, Tab runs the highlighted command as Enter does, so "file" then Tab
  opens Go to File.
- A pane that waits for data draws it as soon as it arrives. On a tour page a graph could stay on
  "Laying out the graph…" and a pinned symbol on "Looking up…" until the pointer moved; any pane
  newly opened on a page could miss its update the same way.
- The pane pickers draw the caret right after the query, as the file picker does, instead of
  after a gap.
- Typing in the pane pickers (Add to page from the tray…, Go to pane…, Move pane to…) no longer
  loses letters: keys the code view binds (`r`, `u`, `n`, `w`, …) went to their commands instead
  of the query, so "tree" became "tee".
- Find panes from the keyboard: **Focus Pane…**, **Move Pane to…** and **Bring Pane from Tray…** in
  the command palette (every pane, with its place; choosing reveals it), and a Ctrl-Tab switcher of
  recently focused panes (hold Ctrl, Tab to step, release to switch; one press returns to the
  previous pane). Ctrl-Alt-= / Ctrl-Alt-- / Ctrl-Alt-0 grow, shrink and reset the focused pane's dock.
- Go to file: Right arrow opens a preview of the highlighted file beside the list (Left arrow
  closes it), in the theme's colours with leading imports folded into one row; images are drawn
  and binary files named. New setting `picker.preview` (on; off in Lightweight mode).
- Pinning makes a pane of its own: Cmd-K P (or the Context pane's Pin button) pins the symbol
  under the caret or pointer directly under the live Context pane, and Cmd-K Shift-P (or the ↓
  beside Pin) pins it to the bottom bar. A pin shows the symbol and its kind, its line, its
  definition and its references, which open with a click and are recorded in history. Pins move
  between the right dock and the bottom bar by dragging, close with × (Cmd-K Z brings one back),
  and are saved with the layout. A pin remembers its file's text and says in its header when the
  file has changed since it was taken. In the bottom bar a pin is wide, with references in
  columns by file. The Context pane no longer freezes in place; it is always live.
- The Pins pane stretches and springs back at its ends on a trackpad, like the other side panes.
- A pull request whose details the host couldn't give (and that had no saved copy) now shows in
  the header band as that pull request, not as a commit. Worktrees remember what they were
  opened for as data, so recents and links read it back even if the wording changes; worktrees
  opened by earlier versions still read as before.
- A `chorong://` link whose Before commit no longer exists opens with the merge base again when
  the link spells the commit in upper case.
- Copy chorong:// Link in a pull-request view (or a Before/After comparison) now includes the pull
  request number, its head, the comparison's Before and the side you are reading
  (`pr`, `rev`, `base`, `side`; provisional). Opening such a link starts the same comparison at
  your place. A link to a pull request Chorong has not fetched asks first ("Open pull request #123
  of github:acme/api?") and fetches only if you choose Open. A Before that is not available, a
  pull request that moved on and a version lost to a force-push each say so.
- The question before fetching a pull request from a link also offers "Open your worktree at
  this commit" when a folder of yours for that repository is at the link's commit: it opens
  there with the same comparison and fetches nothing.
- The graph popover's choices (`graph.json`) and Open Remote's last project and "Prepare and
  index" choices (`remote.json`) are saved atomically, so a crash mid-save no longer loses
  them, and a save that fails is reported on the terminal instead of ignored.
- Query fields in the pickers, the command palette and project search take Cmd-A (select all),
  Cmd-C, Cmd-V, Option-Backspace (delete a word) and Cmd-Backspace (clear).
- The remote header band always says how old saved pull request details are ("details from 3
  days ago"), including in windows reopened from Open Recent, and shows "offline" when the
  latest pull request check could not reach the host, until a later check works.
- Windows on a remote repository's cache worktree show a header band: the pull request number,
  title, author and state (or the project and branch or commit), "3 behind main" with a rebase
  note, and "main fetched 12 min ago" or "offline · last fetch 2 h ago". The relative times
  refresh when you return to the window, not on a timer. A "PR updated: new head … · Update"
  banner is ready and appears once the backend can check the head.
- In those windows, Open in editor becomes Open on GitHub / Bitbucket, at the current file and
  line on the side you are reading.
- Open Remote…: while an open runs with the form hidden, the status bar shows its progress and
  clicking it brings the form back; a Data Center URL shows an editable, checked clone URL.
- A failed Open Remote (or prepare) now says what went wrong in one short sentence, with a closed
  **Details** line under it. Opening it shows the full git command and the end of its error
  output (with any credentials removed) and a **Copy details** button. The Settings sections
  show a finished action in a green success notice instead of the yellow warning one.
- Open Remote has a **Prepare and index** checkbox (checked by default, remembered per project).
  Checked, the new window asks for the project's approval when it has none or the recipe or a
  build file changed (the panel lists the exact commands, the new or changed build files and
  where they run), runs the project's prepare steps in the background with progress and Cancel
  in the status bar, then builds the index. A failed step shows its output with Retry. Unchecked
  opens read-only, syntax only, and runs nothing. Esc in the approval panel opens without indexing.
- A failed prepare no longer stops indexing: the index is built anyway and reads `partial
  (prepare failed)`, and the failure (step, output tail, log path, **Retry**, **Dismiss**) is a
  notice that does not block the window. A successful Retry rebuilds the index cleanly, even if
  the partial build is still running. **Update** on a pull request asks again when the head
  moved: approval if the build files changed, prepare only if its last run no longer covers
  them, then indexing.
- A closed File Tree, Outline or Context pane can be brought back: Toggle File Tree (Cmd-K B),
  Toggle Outline Panel (Cmd-K O) and the new Show Context Pane open it again in its usual place,
  or on the current page in a focus layout.
- Opening a pin, maximising and restoring, and paging focus layouts no longer redraw every
  pane: a Cmd-K chord, a notice, an index answer and indexing progress now redraw only what
  shows them, and pane headers and the status bar are reused while they stay the same.
- A saved layout reopens each editor pane at its caret, not just its file.
- Chorong reads only regular files up to 16 MB from a repository, and refuses a symbolic link that
  leads outside the opened folder. A link, index or symlink naming a device, a pipe, a huge
  file or a file such as `~/.ssh/id_ed25519` now shows a message instead of hanging the viewer
  or displaying it. The CLI, graphs and investigation checks follow the same rule.
- Links are checked more strictly: `rev` and `branch` must be a commit id or a valid git branch
  name, and **Copy git switch command** quotes the name, so a crafted link can no longer put shell
  commands into what you paste.
- Search results in files that start with a byte order mark point at the right place.
- Regex searches that can match empty text or single bytes no longer highlight half a character
  in lines with CJK or other multi-byte text.
- Cmd-Shift-F pressed again while searching (a quick double press works) fills the window with
  search: results on the left, the selected result's file previewed on the right. Again, or
  closing search, restores the layout.
- Search: the error from a failed search (an invalid regular expression, say) no longer stays
  on screen while the next search runs; the panel says "Searching…" instead.
- Settings has three new sections, **Projects**, **Cache** and **Accounts**. Projects edits how
  each remote project is cloned (HTTPS or SSH), how many worktrees it keeps and automatic
  prepare (available once the recipe is approved), shows its approval, and removes a project
  after saying what goes. Cache shows the disk use of every clone and worktree (measured when
  you open it) and clears one worktree, one project or everything after a confirmation. Accounts
  shows each host's token source, checks a token on request and adds or removes tokens in the
  keychain from a masked field; without a keychain it names the environment variable to set.
- A pane can span a whole edge of the code area or a focus page (a full-width row at the
  bottom, say): drop it on the slim targets along the edges while dragging, pick `Whole width,
  bottom` (`Code area width, bottom` in the normal layout) in Move Pane to…, or press Shift-arrow in Layout mode at that edge.
- Split editors: `v` and `s` in Layout mode (also Cmd-K with an arrow key) split an editor pane.
  Each editor pane keeps its own file, caret, folds, highlights, scroll position and back/forward
  history. Context, Outline, the status bar and commands follow the active editor (the pane you
  last clicked or moved focus to), and files open in it.
- Compare Side by Side: each column has a chain icon beside its side chip (a broken chain when
  unlinked); click it to link or unlink (`Shift-B`). When a linked column follows the other, the
  declaration it lands on is highlighted briefly (`compare.follow_highlight`: fade, outline or
  off).
- Opening another file in a split, from the Changes list or anywhere, loads it in both columns
  instead of ending the pair.
- `b` now switches the whole window: every code pane not in a split flips, and files opened
  afterwards open on that side.
- Switching Before/After (`b`) keeps the view where it was: the caret's line stays at the same
  place on the screen (or the top line stays at the top if you had scrolled away), instead of
  jumping to the middle. Files in the focus layout's tray switch when you bring them back, and
  come back where you left them.
- The step banner above the code springs back at its ends like the panes, its bottom edge
  drags to make it taller or shorter (double-click for the default), it is never taller
  than its text, and a More chip shows when text is hidden below its fold.
- More blocks fold, from the syntax: anything between `{}`, `[]` or `()` over three lines, Python's
  indented blocks, and runs of imports. Only function and method bodies start folded.
- Left and Right fold and unfold as in a file tree: Left folds the block on the selected line or
  moves up to the block holding it; Right unfolds, or steps into an open block.
- Code text can be selected and copied. Drag selects any span, across lines, and scrolls the pane
  when dragged past its top or bottom; Shift-click extends; a triple-click selects the line; a
  double-click still toggles the word's highlight and now also selects it. Cmd-C (Ctrl-C off
  macOS) copies the selection, or the caret's line when nothing is selected; Cmd-A selects the
  whole file; Esc clears the selection before it removes highlights. **Copy with Location**
  (Cmd-Shift-C) copies a `path:first-last` header and the text in a fenced block tagged with the
  file's language. Copies are the file's own bytes, tabs and folded lines included. The selection
  is drawn in the theme's selection colour over any tint. On macOS an Edit menu holds Copy, Copy
  with Location and Select All.
- Measured in the viewer (release build, Linux under Xvfb, default settings, a 2.1 MB Rust file of
  54,026 lines, unfolded, two runs, `CHORONG_FRAME_LOG`): frames while dragging a selection take 3.8
  to 3.9 ms median and 5.2 to 5.8 ms at the 95th percentile, and while auto-scrolling 4.3 ms
  median and 6.1 to 6.5 ms p95, against 4.0 to 4.4 ms median (5.6 to 6.2 ms p95) for moving the
  caret with Up and Down and 4.8 to 5.2 ms median for clicking lines. Scrolling with a selection
  on screen costs the same as without (3.6 ms median either way). With nothing selected no
  selection work is done; with one, only the rows being drawn are checked.
- Tours: a chunk's numbered strip sits above its own excerpt, so several chunks in one function
  each get theirs (they had stacked above the function's first line), and a step opens with
  chunk 1 in view after its folds open.
- Tours: a highlighted chunk tints its declaration faintly and its excerpt darker, like a
  step's own anchor. A step with several chunks draws only them (its anchor names the stop),
  opens on chunk 1, and lists them under its note with their whole labels; a click on a row
  goes to the chunk.
- Tours: your own highlights no longer remove a step's chunks (`h` inside a chunk, Esc, Clear
  Highlights), and `n` / `N` follow the group the caret is on.
- Tours: no landing outline while a step is open in its file (it marked wherever the caret fell,
  not what the step is about). A step's `excerpt` or `from` / `to` lines get a stronger tint
  inside the step's tint.
- Tours: the line where a step puts the caret keeps the step's tint instead of the selected-line
  colour, until you move the caret.
- Tours say "step" on screen, as the file format does: "step 3 of 7" in place of "stop 3 of 7".
  Call stacks still say "frame".
- Bring Pane from Tray stays open after bringing a pane in, so a focus page can be filled pane
  by pane; Escape ends it.
- The file tree pane is now titled "Tree" (it was "Files"), so it no longer shares a name with
  file search.
- Show Changes and Show References now take the keyboard from the search box; before, keys
  stayed in Search and fell into the Changes list once search closed.
- A failed Update of a pull request is no longer silent when it needs a command run (such as
  signing in to the host): the command shows in a banner with Copy command.
- A pinned context pane whose pin changed no longer shows the slower result of its old pin,
  and an older read of the index cache can no longer replace a newer one.
- The Open Remote boxes take Cmd-A (Ctrl-A) and copy the selected text, like the pickers.
- Visual refresh of the panes: the Files pane is a tree (folders first, compact folder chains,
  file icons by type, indent guides, the open file revealed and highlighted); the Outline shows
  a kind glyph in the kind's syntax colour, indent guides, bolder containers and an accent bar
  on the declaration holding the caret; the context pane has section headers with qualifiers, a
  declaration card, a file row and signature for the definition, references grouped under file
  rows with count pills, and styled "needs an index" and stale-index notes; the status bar uses
  icons, spacing instead of `·`, an index dot that differs by shape and colour, and a Critical
  Lens chip.
- One rule for where files open, in every picker and list: Enter replaces the active code
  pane, Cmd-Enter opens a new pane to the right, Cmd-Shift-Enter below, Alt-Enter a new tab;
  Alt-click and Alt-Shift-click do the same with the mouse, and Cmd-Alt-click opens a definition
  to the right. Outside a focus layout Cmd-Enter now splits instead of adding a tab.
- On macOS, Cmd-N opens project symbols and Ctrl-Cmd-N the file picker, as in some Zed keymaps;
  Cmd-T and Cmd-P still work.
- The viewer's panes now sit in a layout: pane headers (minimise, maximise, close, drag to move),
  left, right and bottom docks, dock toggles in the status bar, and keyboard Layout mode (`w`).
  The layout is saved per repository (off the UI thread) and restored. Cmd-K O now folds the
  Outline and Cmd-K B toggles the left dock. Cmd-K P still toggles the viewer's pin. The first
  editor pane shows the open file; split editors follow. `debug.frame_overlay` shows frame times.
- `cho PATH` is shorthand for `cho open PATH` (`cho .`, like `zed .`). On Linux a second
  `chorong-app PATH` hands the path to the running viewer instead of starting another one.
- Export for sharing (`cho graph --share`, the viewer's graph export and **Copy chorong:// Link for
  Sharing**) refuses a repository that would be `local:/absolute/path` (outside the home folder,
  no remote) and says to add a remote or pass `--repo`. Your own links are unchanged.
- On Linux the viewer picks an installed UI font (Inter, Cantarell, Ubuntu, Noto Sans, DejaVu Sans,
  Liberation Sans) instead of GPUI's `.SystemUIFont`, which isn't installed on typical systems and
  makes every text run repeat a failed lookup. macOS is unchanged. The shared picker is the
  `cho-fonts` crate.
- Read-only GPU viewer (GPUI) for files and folders, with Zed theme support and a warm Chorong
  Dark / Chorong Light pair.
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
  Chorong's cache and opened in the browser.
- `chorong://` links: remote links open a known clone of the repository (or offer the web page);
  Copy chorong:// Link (Cmd-K C) and Copy chorong:// Link for Sharing.
- A banner for links made at another commit: View at that commit (read from git,
  read-only), Open worktree, Rebuild graph, and the `git switch` command to copy.
- Registered as the `chorong://` handler (Linux `.desktop` file and install script; macOS
  `.app` bundle metadata); on Linux a second launch hands its link to the running viewer.

### Graph
- The `cho open` command on a graph page stays one argument for a branch name with a `'`, DOT
  output keeps branch and path names in its header on one line, and a package manifest that is a
  symbolic link out of the repository no longer names a package.
- **Graph pane**: **In a pane** in the Show graph popover draws a declaration's neighbourhood
  inside the viewer, laid out by Chorong: solid lines from the index, dashed claimed, dotted red
  broken, dashed boxes for what is not in your code. Fit, 1:1, Open in browser, and a click on a
  box opens its declaration. An interim drawing; the HTML page is unchanged.
- `cho graph --svg OUT` writes the same drawing as SVG (`--group file|package`, `--top-down`).
- A graph built from a partial index (after a failed prepare) or from an earlier index whose
  last rebuild failed now says so in its notes, as a stale index already did. The viewer's
  branch blast radius words a deleted file as "Deleted in the diff", like `cho graph`.
- Graph nodes can no longer be dragged by accident: dragging anywhere pans, and lines stay on
  their routes. A click still selects a node.
- Graph pages start small and readable: tests are hidden (except in a blast radius), only
  lines that lead to what you graphed are drawn, and past 10 callers a "+N more" box shows 10
  more, then all of them in a focused view with Back. Toggles bring back tests and other
  lines.
- Lines follow the layout's right-angled routes instead of crossing; package level shows
  closed package boxes, with line thickness for references and a count of the things used.
- Clicking a line, symbol, file or package lists what it uses and what uses it, with
  reference counts and quiet bars, grouped by package for files and packages.
- A big graph opens at a readable size on what you graphed, with an overview map; Fit still
  shows everything.
- Pages load the drawing libraries from one copy in Chorong's cache folder: about 150 KB instead
  of about 2 MB. `--share` and the new `--inline` keep everything inside the page.
- A repository package with references the index could not resolve is drawn once, with the
  unresolved part inside it, instead of twice.
- Graphs open in Safari again: a page in Chorong's cache folder loads its libraries from a copy
  beside it rather than from a parent folder, which Safari refuses for local files. A page
  written outside the cache folder (`--html OUT`) now has the libraries inside it. Build an
  older graph again to fix it.
- `cho graph`: neighbourhood, file, package, outbound closure and local-branch blast radius
  from the semantic index, as JSON, DOT or a self-contained offline HTML page (header with
  worktree, branch, commit and dirty state; `chorong://` links on every node).
- `chorong://` links accept `branch` and `dirty=1`; `cho link build` has `--branch` and `--dirty`.
- Links name the repository by its remote (`github:`, `bitbucket:`, `bitbucket-dc:`), or
  `local:~/…` without one; never the absolute path under home. `cho graph --share` drops
  branch, dirty state and worktree name. Graph links carry `from=graph`.

### Semantics
- The TypeScript and Python indexers run from Chorong's own copies, installed once into Chorong's cache
  (`npm install --ignore-scripts`) and started with `node`, instead of `npx` in the repository: a
  package of the same name in the repository's `node_modules`, its `.npmrc`, or its package
  manager no longer chooses the program. For pnpm workspaces Chorong lists the packages from
  `pnpm-workspace.yaml` itself instead of running `pnpm ls` in the repository (same index).
- scip-python runs with `PYTHONSAFEPATH=1`, so Python imports nothing from the project folder; a
  project's `.venv` (as `uv sync` makes it) goes first on `PATH`, so its packages' imports
  resolve.
- Chorong started from Finder or the Dock can index TypeScript and Python: `node`, `npm` and `uv`
  are found in the login shell's `PATH` and the usual install places (Homebrew, nvm, volta, fnm,
  `~/.local/bin`, `~/.cargo/bin`) and saved as settings. Not found: the error lists every place
  searched.
- In pnpm, npm and yarn workspaces whose packages export built declarations (`dist/*.d.ts`),
  go to definition, references, hover, the context pane and the reference graph now reach the
  source declaration from other packages. Chorong maps a built declaration to its source by the
  declaration map, tsconfig `outDir`/`rootDir`, the manifest's `source` field or source export
  condition, or a `dist` to `src` path swap (bundled declarations through the source entry's
  re-exports), and links only when the index has a definition with the same name chain in that
  file. Such links are labelled "mapped (unverified)" with their method.
- Repository configuration can no longer make Chorong's git commands run a program through a filter
  or diff driver whose name has capital letters or an `=` (such as `[filter "Evil"]`). More
  program-running settings are switched off for repositories Chorong reads (`core.gitProxy`,
  `remote.<name>.uploadpack`, merge drivers, `pager.<command>` and others).
- Two indexing runs of the same project at once (two Chorong windows) no longer make one of them
  fail, and index status writes no longer collide.
- Indexers no longer inherit your whole environment. Build scripts and npm tooling see only
  what a build needs (paths, toolchain, compiler, proxy and locale settings), so tokens and keys
  exported in your shell, such as `GITHUB_TOKEN`, `AWS_*` or `NPM_TOKEN`, stay out of their
  reach. The trust prompt now says plainly that trusted code runs as you.
- Indexing runs at a lower CPU priority (`nice -n 10`, with everything it starts), so opening
  and reading files stays responsive while the first index builds.
- TypeScript files over 1 MB are now indexed (up to 8 MB). scip-typescript skipped them silently,
  so a large generated parser or a one-file type checker had no definitions or references.
- Definitions, references and hovers on files with very long lines (minified or generated code)
  no longer slow down with the length of the line: column conversions scan at most 1 KB.
- Nothing indexes or prepares until you click, even in a folder you trust: files read and
  highlight at once. A banner (a card at the bottom right) says what a build runs, whether the
  project's own code runs and how long the last build took, with **Build index**; an untrusted
  folder's click asks for trust first. A stale index offers **Refresh** and **Keep fresh this
  session**. Remote projects get **Prepare and index** and **Index only**, with the recipe's
  commands; an unapproved one shows the approval panel first. Lightweight mode still shows the
  banner and starts nothing by itself.
- A local TypeScript project without `node_modules` is now indexed, with a warning that imports
  between packages won't resolve until you install the dependencies.
- TypeScript indexing no longer runs out of memory on large monorepos: Chorong gives scip-typescript
  an 8 GB Node heap by default (the `index.node_memory_mb` setting; a `NODE_OPTIONS` heap size wins). Before, an installed trigger.dev
  checkout died with "JavaScript heap out of memory".
- The TypeScript and Python indexers are pinned (`@sourcegraph/scip-typescript@0.6.0`,
  `@sourcegraph/scip-python@0.6.8`) instead of fetching the newest release on every build.
- TypeScript indexing now works in pnpm workspaces that have no root `tsconfig.json` (most
  Turborepo monorepos): Chorong passes `--pnpm-workspaces` to scip-typescript when it finds
  `pnpm-workspace.yaml`. Before, the index failed with "missing tsconfig.json".
- Indexing progress never reads "4 of 3" and shows control characters in step names as escapes.
- Trust now follows the repository, not just the folder: Chorong records the trusted folder's git
  remote, and if a different project later sits at the same path, it asks again before running
  anything, saying which remote changed. Folders trusted by older versions keep their trust.
- Built-declaration links now read tsconfig files with a comment after a trailing comma (`"rootDir":
  "src", // …`), which previously made the whole file unreadable. A pathological workspace glob in
  `package.json` or `pnpm-workspace.yaml` can no longer stall loading the index, and a `types` or
  `source` field that names a pattern or a declaration file is ignored instead of linked.
- `cho hover --peek` and `Semantic::peek_definition` now answer for fields, properties, parameters
  and locals: the enclosing declaration with the member's line(s) marked (`kind: "enclosing"`,
  `marked_range`). `no-declaration` is left for definitions that nothing in the outline contains.
- SCIP indexes for Rust, TypeScript and Python: definition, references, symbol search. Never
  guesses by name; stale indexes are labelled.
- `cho hover FILE LINE:COL`: the indexer's signature (inferred locals included), docs,
  definition and the index's commit and status. Says so when the index has no signature.
- Hover has canonical names: `name` (the index's display name, else the symbol ID's last
  descriptor, else the source text for locals) and `local_text` (what the occurrence says, e.g.
  `plus` for `import { add as plus }`).
- Definition peek: `cho hover --peek` (and `Semantic::hover_at_with_peek`) gives each
  definition's full declaration range from the Tree-sitter outline, or says why there is none.
  Computed on request only, with a per-file outline cache.

### CLI (`cho`)
- Opening a remote repository or pull request no longer fails with "refusing to write outside
  Chorong's cache" when the cache's own path goes through a symbolic link (for example a linked
  home or projects folder, or macOS temporary folders under `/var`).
- Changes against the working folder show a file over the 16 MB source limit (or one Chorong can't
  read, such as a symbolic link out of the folder) as modified at file level, with reason
  `too-large` or `unreadable`, instead of as deleted.
- `cho changes --worktree` compares the base (`--base REV` or `--merge-base TARGET`) with the working
  folder instead of a commit: edited, staged, deleted and untracked files count, gitignored ones do
  not. Each hunk and declaration touched by uncommitted work carries `"uncommitted": true`
  (additive at version 1; absent means false).
- `cho changes --base REV | --merge-base TARGET [--head REV]` lists the files and declarations
  that differ between two commits (added, changed, removed, or ambiguous when a name has
  several declarations), read from the object store without touching the checkout. `--json`
  gives a stable shape with byte ranges on both sides.
- The CLI is published on crates.io as `chorong`: `cargo install chorong` installs the `cho`
  command, and `cargo install chorong-app` the viewer. The investigation JSON Schema moved to
  `crates/cho-cli/schema/investigation.schema.json` (`cho schema investigation` prints it).
- Changes and graphs keep hunks for files whose names contain spaces, quotes or non-ASCII
  characters, and for changed lines that start with `--` or `++`; a `diff.noprefix` setting no
  longer hides every hunk.
- `cho index` prints progress lines on stderr (the indexer's current package or phase, `n of m`
  where the tool says, and the elapsed time), and Ctrl-C stops the indexer cleanly and keeps the
  previous index.
- A failed indexer's report keeps Node's reason (`FATAL ERROR: …`) instead of losing it behind a
  long native stack trace, so the out-of-memory hint naming `index.node_memory_mb` shows again.
  `cho index`, `cho index status` and the viewer lead with the reason.
- `cho open` prepares and indexes only with `--prepare`; it no longer does so on its own for
  projects with auto-prepare on. `cho prepare auto` was removed and says so.
- `cho PATH --compare REF` (and `chorong-app PATH --compare REF`) opens straight into a comparison
  with that branch, tag or commit, as Compare Against… does.
- `cho open --name NAME` (and `chorong-app --name`) titles the window `NAME — repo — Chorong`, so a
  window an agent opened is easy to find; files opened together without a name are titled
  `repo — 3 files — Chorong`. `--dry-run` prints the command with quotes where a shell needs them.
- `cho a.rs b.rs …` (and `cho open`, `chorong-app`) opens several files in one window rooted at
  their repository: the first on a focus page, the rest in the tray. `cho -` reads the paths
  from standard input, so `git diff --name-only | cho -` opens what you just changed.
- `cho open REPO … --view` opens the fetched worktree in the viewer. A pull request's details
  are kept with its worktree, so reopening it offline still shows its title, branches and
  target (marked stale).
- `cho find` and investigation checks find Swift operators whose names contain dots, such as
  `.+.`, instead of reporting them missing.
- `cho link parse` prints the provisional `pr`, `base` and `side` of a `chorong://open` link, and
  `cho open --view` on a pull-request link opens that pull request, using the link's `base` as
  Before unless `--base` is given.
- The prepare approval prompt shows recipe arguments that contain control characters (terminal
  escapes, newlines) or right-to-left overrides as escaped `$'…'` text, so a repository's
  `.chorong/prepare.json` can't make the prompt show a different command than the one that runs. A
  step folder with such characters is refused.
- A prepare step that prints without end is stopped once its log passes 64 MB, instead of filling
  the disk until its timeout; the folders you opened recently are no longer forgotten because one
  entry in `recent.json` was malformed.
- Prepare remote worktrees before indexing: `cho prepare show|approve|set|run|auto|clear` and
  `cho open … --prepare`. A recipe comes from the project, the repository's
  `.chorong/prepare.json` or a lockfile suggestion; one approval covers prepare and indexing and
  is asked again when the recipe, a lockfile, a `package.json`, a `build.rs` or a proc-macro
  crate changes. Steps run without a shell, with a timeout, without host tokens, only in
  Chorong's own worktrees. A failed prepare still indexes, and the index reads `partial`.
- `cho index` on your own folder names the install command to run when `node_modules` is
  missing; the new `prepare.auto` setting (off in Lightweight mode) gates automatic prepare.
- Open GitHub and Bitbucket branches, pull requests and commits without a clone of your own:
  `cho open github:acme/api --pr 123` (or a URL) fetches into Chorong's cache, places a worktree
  and prints Before (the merge base with the target branch, or `--base`), After and how far
  behind the target it is. New `cho project`, `cho pr`, `cho auth` and `cho cache`
  commands; `cho graph --pr REPO N` shows a pull request's blast radius. Tokens live in the OS
  keychain, with environment variables as the fallback.
- A hand-edited `projects.json` is checked on load as strictly as `cho project` checks new
  entries (clone URLs, recipes, worktree cap, auto-prepare approval), naming the bad entry.
  Repeated folders in the recent list are shown once, and unsafe worktree names in a
  repository's cached state are dropped.
- Remote-source errors are one short line naming the host or repository and the cause, such as
  "Can't reach github.com (connection refused)". `--verbose` shows the git command and its
  output, and `--json` adds the error's `kind` and `detail`. Neither shows tokens or URL
  credentials.
- An installed investigation skill carries a version stamp, and `cho investigation check` and
  `list` say when a repository's copy (under `.claude/skills` or `.cursor/skills`) is not this
  cho's, with the command that updates it.
- `cho index` no longer says a TypeScript package "has no node_modules" when they are hoisted
  to the repository root, or when the package declares no dependencies (the viewer already
  accepted both).
- A `.chorong/prepare.json` that is a folder is ignored with a note, like an invalid one, instead of
  failing prepare and indexing.
- Git failures that merely mention a pull request numbered 401 or 403 are no longer taken for
  an authentication failure.
- `cho graph --pr REPO 0` is refused as not a pull-request number.
- `cho symbols` now finds TypeScript and JavaScript symbols. scip-typescript records no display
  name, so the search matched nothing; it now uses the name in the symbol's identifier.
- `cho open` refuses targets with control characters (also when percent-encoded in a URL), clone
  URLs whose bracketed host starts with `-`, and branch names with a part ending in `.lock`, as git
  does, instead of failing later or passing them on.
- Prepare suggests `uv sync --frozen` for a repository with `uv.lock` (after the JavaScript
  install when there is one), and `uv.lock`, `pyproject.toml` and `.python-version` are trigger
  files: a change asks for approval again.
- Headless commands with JSON output for people and agents: outline, find, show, highlight,
  search, index, definition, references, symbols, investigations, links, editors, themes.
- Self-describing formats for agents: `cho formats`, `cho schema`, `cho skill`.

### Investigations
- Tours can mix code with **action steps** and **diff panes** (optional additions to format
  version 2). An action step is an instruction with commands: its card shows each command with a
  **Copy** button, in the editor and as a focus page's main pane. Chorong never runs a command.
  A `diff` pane shows how one file changed between two revisions (or a revision and the working
  tree) with the change marks of Compare against…; a revision the repository doesn't have is a
  card saying so. `cho investigation check` reports actions as `action` and an unresolved
  revision as broken, and the agent skill teaches both.
- **Close investigation** is in the ⋯ menu of the Tour, Call stack and Comparison panes, and it
  closes those panes too instead of leaving them empty.
- The Comparison tab opens with a taller bottom bar, so a comparison's rows fit without
  scrolling. Choosing another variant clears a code pane's side strip left from the old row,
  instead of showing what that side no longer is.
- The Comparison tab keeps the row whose code is shown highlighted after a row's page closes,
  and the note strip and status bar name a row by its own number (`row S1`, `row 2`), as the
  table does.
- The viewer shows format version 2 comparisons in a **Comparison** tab in the bottom bar: one
  row per step, a column per side, and a result chip (same, differs, missing) that differs by
  shape as well as colour and says whether Chorong derived it or the author claimed it, and when
  every side runs the same code. Setup rows have their own group, callouts ("Paths split
  here") follow their row, and the counts in the header filter the rows. Enter on a row opens
  a focus page with one code pane per side, each at its anchor and tinted.
- **Open Comparison as Tour** (Cmd-K Shift-C) steps through the same pages, one per row, with
  the rows and their results in the rail.
- A side's **variants** have a picker in the Comparison tab and the rail (`v` for the next);
  a tour's **branches** have a **Path** picker in the Tour pane and the rail (`b` for the
  next). Steps a branch does not reach stay listed, muted with the reason, and stepping skips
  them. Chorong remembers the choices per investigation.
- Bold, italic, `code` and links in an investigation's text now render (instead of showing
  `**`, `*` or backticks) in the strip above each highlighted chunk and its tooltip, the
  numbered list of chunks under the step note, the summary bullets and "could not be traced"
  list, and in a comparison's callouts, missing-side reasons, variant and path notes and row
  tooltips. Links there show their real target and are not opened from these places.
- Format version 2 in the Tour and Call stack panes and on pages: several highlighted chunks in
  one file, each with a numbered note strip and `n` / `N` between them; nodes that are not in
  the code; setup stops numbered S1, S2…; the author's evidence and runtime values shown as
  claims; short stop names, summary points and a "could not be traced" list. On tour pages a
  view can pin a symbol, show the outline, the call stack or the file tree, and draw a graph of
  the stop (the graph pane, from the index).
- Investigation format version 2 (`"chorong": 2`) is read and checked, adding per-step views
  (panes and an arrangement) and several highlights per editor pane, each with a note. Anchors
  can give a range by text (`from` and `to`).
- Version 2 also adds external nodes that aren't code, and setup and call phases (S1, S2…;
  1, 2…). It adds the author's evidence (`read` or `inferred`), runtime values and short step
  names, plus summary bullets and a "could not be traced" list.
- Comparisons (`kind: comparison`) align two or more paths row by row, with variants of a
  side and tour branches whose skipped steps are "not reached".
- `cho investigation check` reports all of these, along with each step's number, call depth
  and callers. Version 1 files load and check exactly as before, and version 2 fields are
  rejected in them.
- Investigation format: a highlight has a `name` (its two-to-five-word label, on the strip
  above the chunk) and a `note` (a sentence or two of commentary, under the label in the
  step's list of chunks). A file without `name` reads its highlight `note` as the label, as
  before.
- `cho investigation list` lists the investigations in a repository's
  `.chorong/investigations/` folder, with title, kind and pinned commit, or why a file is invalid.
- `cho investigation check --at rev` checks anchors against the investigation's pinned
  commit, read from git, instead of the working tree; check output now says how the checkout
  relates to that commit (`drift`).
- `cho investigation check` accepts an `https://` URL (4 MB limit, 15-second timeout, no
  credentials, redirects only to `https://`).
- Investigation notes render `**bold**` and `*italic*` (also `__bold__` and `_italic_`) in the
  step note strip, the action card and external cards, instead of showing the asterisks.
  `snake_case_names`, unmatched `*` and anything inside `code` stay as written; links and raw
  HTML behave as before.
- A step's note above the code (and an external stop's card) wraps word by word at the pane's
  width, as the action card already did, instead of running off the side.
- Go to file (Cmd-P) starts with a **Tour** chip while a tour is open: only the tour's files.
  Backspace on an empty query or a click lists every file; Tab (or `tour` and Tab) or Alt-T
  puts it back.
- The investigation skill (`cho skill investigation`) now writes format version 2, and
  `cho formats` reports version 2 (version 1 files still load). It teaches choosing between a
  call stack, a tour (including several files side by side with highlighted chunks), a
  comparison and a diagram; views, highlights, `from`/`to` ranges, external nodes, phases,
  evidence, runtime values, comparison results and branches; and getting declaration names,
  uses and branch changes from Chorong instead of grepping. Every example in it is checked
  against the format by the test suite.
- The investigation skill says how each piece of text is shown: step, action and external notes
  are inert Markdown (`code` and links only), points, callouts and highlight notes are plain
  text, highlight notes are one-line labels, and `summary` is not shown in the viewer (so
  assumptions go in `points`).
- `n` / `N` move between a tour step's highlighted chunks with the keyboard still in the Tour or
  Call stack pane, where it is after opening an investigation or clicking a step; before, they
  worked only once the code had the keyboard.
- Step keys: `j` / `k` (and Down / Up), Enter and Esc in the Tour and Call stack panes, and
  Alt-] / Alt-[ / Alt-Home / Alt-End to step from anywhere while an investigation is open. The
  cheat sheet (`?`) has a Tour and call stack group.
- In the comparison list, the row the arrow keys are on now has the lighter fill and accent
  bar that tour and call-stack lists use, so it looks different from the open row, which keeps
  the full fill.
- **Open Tour as Focus Pages** (Cmd-K Shift-T): a focus layout with a page per tour stop, laid
  out from the stop's named arrangement (or the code plus Context). The page strip shows the stop
  badges, the tour rail rides along on every page, paging steps the tour and stepping the tour
  pages, Reset Focus Page undoes your rearranging, and Exit focus keeps your place. An anchor
  that is not found is a card that says why.
- The viewer opens tours and call stacks. **Open Investigation…** lists the repository's
  `.chorong/investigations/` and recent ones; **Open Investigation File…** opens any `.json`; and
  `chorong://investigation?src=…` links open in the window for their repository. A tour opens in
  the Tour pane above the file tree, which it filters to the tour's files; a call stack opens
  in the bottom bar with verified, claimed and broken frames told apart by shape. The current
  step opens at its anchor with its lines tinted and its note above the code.
- When the checkout has moved on from the investigation's commit, Chorong asks whether to use
  the pinned commit (read from git) or the working tree. Notes are inert Markdown: no remote
  images or HTML, and links show their real target.
- An investigation can be opened from an `https://` URL: the file alone is fetched (https only,
  size-limited, with a timeout and no credentials) and its host is shown.
- A JSON format for call stacks, tours, diagrams and PR overlays, checked against the code
  (`verified`, `claimed`, `broken`).
