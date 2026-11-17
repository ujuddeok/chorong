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
