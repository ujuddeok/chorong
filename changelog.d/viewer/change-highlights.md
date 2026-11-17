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
