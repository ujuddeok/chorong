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
