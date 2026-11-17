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
