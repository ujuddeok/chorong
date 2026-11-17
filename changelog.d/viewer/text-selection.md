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
