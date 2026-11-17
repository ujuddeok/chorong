- JSON and JSONC files fold: every object and array over three lines, from the syntax tree, with
  the usual fold marks, Space / Enter / Left / Right on a line, and Fold All / Unfold All (Cmd-K 0,
  Cmd-K J) to collapse or expand the whole file. JSON files open unfolded. The fold pass is one
  extra parse per JSON file on open, in the background: about 70 ms for a 1 MB file in a release
  build (highlighting the same file takes about 200 ms). Files up to 256 KB fold before they
  show; a larger one shows at once and its fold marks appear when the parse ends (a Fold All
  pressed meanwhile runs then). New setting `editor.json_folds` (default on, off in Lightweight
  mode).
