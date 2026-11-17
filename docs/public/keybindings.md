# Key bindings

How Chorong's keyboard is organised, and how bindings are defined so they can be remapped
later. The scheme below is implemented from `app/keymap/default.json`; the user guide's table
is generated from that file. Bindings whose feature does not exist yet (project symbols, find
in file, go to line, Hide Tests, open on GitHub, switch theme) are bound
to actions that show "not available yet". Go to definition and references (`d`, `r`) need an
index; references report their count and the context pane lists them.

`Cmd` below means Ctrl on Linux and Windows, unless a row says otherwise.

## Layers

### 1. Cmd + key: go somewhere

Pickers and navigation. Zed's bindings, and VS Code's where Zed agrees.

| Key | Action |
| --- | --- |
| Cmd-P | file picker |
| Cmd-Shift-P, Shift Shift | command palette: every action, with its binding shown (Shift Shift: press Shift twice quickly) |
| Cmd-O | open a file or folder with the system's Open dialog |
| Cmd-Shift-O | symbols in this file |
| Cmd-T | symbols across the project (needs an index) |
| Cmd-F | find in file (in its bar: Enter / Shift-Enter next / previous match, Alt-C case, Alt-R regex, Escape close) |
| Cmd-Shift-F | search the project |
| Ctrl-G | go to line (Ctrl on every platform, as in Zed) |
| F12, Cmd-click | go to definition |
| Shift-F12 | references (focuses the context pane's list) |
| Cmd-[ / Cmd-] | back / forward |
| Ctrl-- / Ctrl-Shift-- | back / forward (Zed's bindings; both pairs work) |

On Linux and Windows, Alt-Left / Alt-Right also go back and forward.

On macOS, Cmd-N (project symbols) and Ctrl-Cmd-N (file picker) also work, as some Zed keymaps
bind them; Cmd-T and Cmd-P keep working.

### 2. Cmd-K, then a key: change the view

Cmd-K starts a chord for view commands. The rule: **a Cmd-K chord changes how you are
viewing, never where you are.** After Cmd-K, a small popup lists the keys that can follow,
so the chords don't have to be memorised.

GPUI drops a pending chord after one second, and the timeout is fixed in GPUI (not
configurable in 0.2.2). Chorong notices a chord dropped without a key (it was pending for at
least half a second and no keystroke arrived) and holds it open itself: the popup stays,
the key context becomes `Chord` (no single-key bindings), and the next key is looked up in
the live keymap as the rest of the chord. A key that completes no binding closes the popup.

| Chord | Action |
| --- | --- |
| Cmd-K L | Critical Lens on/off |
| Cmd-K H | Hide Tests on/off |
| Cmd-K O | outline panel |
| Cmd-K B | file tree |
| Cmd-K P | pin the context in a pane under Context; on a focused pin, unpin it |
| Cmd-K Shift-P | pin the context to the bottom bar |
| Cmd-K M | maximise the focused pane (a focus layout), or exit focus |
| Cmd-K ] / Cmd-K [ | next / previous focus page |
| Cmd-K 1 … Cmd-K 9 | focus page by number |
| Cmd-K Shift-N | new focus page, starting with the focused pane (from the normal layout it starts a focus layout); the tray picker opens to fill it (Tab there adds files; with an empty tray the file picker opens) |
| Cmd-K A | add to the current focus page: the tray picker, Tab for files (files straight away when the tray is empty) |
| Cmd-K R | reset a tour step's focus page to the panes the step declares |
| Cmd-K Shift-T | open the tour as focus pages, one per stop (`chorong::OpenTourPages`) |
| Cmd-K Shift-C | open a comparison as a tour, one page per row (`chorong::OpenComparisonAsTour`) |
| Cmd-K Z / Cmd-K Shift-Z | undo / redo a layout change |
| Cmd-K G | Show graph (see [graph.md](graph.md)) |
| Cmd-K I | build or refresh the index |
| Cmd-K E | open in the external editor |
| Cmd-K W | open on GitHub / Bitbucket |
| Cmd-K C | copy a `chorong://` link to this place |
| Cmd-K T | switch theme |
| Cmd-K 0 | fold all |
| Cmd-K J | unfold all |

### Where files open

One rule for every picker and list that opens a file (see the
[user guide](user-guide.md#where-files-open)): Enter opens in the active code pane, Cmd-Enter in a
new pane to the right, Cmd-Shift-Enter below, Alt-Enter in a new tab. With the mouse, Alt-click is
right and Alt-Shift-click below; Cmd-Alt-click opens a definition to the right. In the add-to-page
picker (key context `AddToPage`) Enter adds to the right and Shift-Enter below. Spanning a whole
edge has no key of its own: drag to the edge, use Move Pane to…, or Shift-arrow in Layout mode.

### Panes and docks

These work from anywhere in the window; the panes themselves are described in the
[user guide](user-guide.md#panes-and-layout). They are in the keymap's `Chorong` context, so they can be
remapped like every other binding.

| Key | Action |
| --- | --- |
| Ctrl-Tab | switcher of recently focused panes: hold Ctrl, press Tab to step, release to switch; a single press returns to the previous pane |
| Ctrl-Shift-Tab | the same, opening on the least recent pane and stepping the other way |
| Ctrl-Alt-= / Ctrl-Alt-- | grow / shrink the dock of the focused pane by 24 px |
| Ctrl-Alt-0 | put that dock back to its default size |

Ctrl is Ctrl on every platform for these, as in Zed. The command palette also lists
**Go to Pane…** (every pane, with its place), **Move Pane to…** (where the focused pane could go)
and **Add to Page from Tray or Files…** (Cmd-K A, in a focus layout). In Layout mode, `p` opens
Move Pane to… and `b` opens Add to Page.

The pickers and the switcher have their own key contexts, `PanePicker` and `PaneSwitcher`:
Up / Down choose, Enter confirms (Shift-Enter brings a tray pane in below instead of to the
right), Escape closes. In Add to Page, Tab (with an empty query or the start of "files")
switches to the file picker in key context `AddToPage`: Enter adds the file in a new code pane to
the right, Shift-Enter below, Backspace on an empty query goes back to the tray. The switcher also takes Left / Right. Letters type into a picker's query
(the code view's single keys do not apply there).

The switcher switches when the modifier it was opened with is released; GPUI reports modifier
changes on every platform. If a release is ever missed, the next frame notices that the modifier
is up, and Enter, a click or Escape work in the meantime. A binding with no modifier (for
example `f6`) has nothing to release, so it switches to the previous pane at once.

### 3. Single keys in the code view: the reading loop

The code view is read-only, so unmodified keys have nothing to type into. They are bound
only when the code view has focus; pickers, search fields and the palette get keys as text.

| Key | Action |
| --- | --- |
| `h` | highlight the symbol under the cursor (the keyboard form of double-click) |
| `n` / `N` | next / previous occurrence in the newest highlight group (a tour step's highlighted chunks are a group too) |
| Esc | clear the newest highlight group; again to clear all |
| `d` | go to definition |
| `r` | references |
| Space | fold or unfold the current declaration |
| `[` / `]` | previous / next declaration |
| `?` | cheat sheet of the current bindings |
| Up / Down | move the cursor line |

After Shift-F12 or `r`, the keyboard is in the context pane's reference list (key context
`References`): Up / Down choose, Enter opens and stays in the list, Escape returns to the
code view.

After Show Changes (or a click on a row), the keyboard is in the Changes list (key context
`Changes`): Up / Down choose, Enter opens and stays in the list, Escape returns to the code view.

In the Show graph popover (key context `Graph`, which has no text field, so single keys are
safe there too): Up / Down choose where the graph starts,
`d` cycles the direction, `+` / `-` change the depth, `s` toggles "for sharing", Enter
builds and Escape closes. Edge kinds are toggled with the mouse.

In the Tour and Call stack panes (key context `Steps`, after opening an investigation or
clicking a step): `j` / `k` and Down / Up move between steps, Enter opens the step and keeps
the keyboard in the pane, `n` / Shift-N (`chorong::NextOccurrence`, `chorong::PrevOccurrence`) move
the code between the step's highlighted chunks, Escape returns to the code view, and `?` shows the cheat sheet with
the Tour and call stack group first. From anywhere while an investigation is open, Alt-] and
Alt-[ (`chorong::NextStep`, `chorong::PrevStep`) go to the next and previous step and open it, and
Alt-Home and Alt-End (`chorong::FirstStep`, `chorong::LastStep`) go to the first and last. Steps
that the chosen tour branch does not reach are skipped.

The Comparison pane uses the same context: `j` / `k` move between rows (over the rows shown,
when a group is folded or a count filters them), Enter opens the row's focus page with one
code pane per side, and `v` (`chorong::NextVariant`) chooses the next variant of the first side
that has any. In the Tour pane, `b` (`chorong::NextBranch`) chooses the next branch. `?` lists
them under **Comparison, variants and branches**.

Vim-style keys are otherwise not bound by default; they belong in an optional keymap once
remapping exists. **The one exception is `j` / `k` (and `v` / `b`) in the Tour, Call stack and Comparison
panes:** those
panes are lists, not the code view, so binding them there takes nothing away.
