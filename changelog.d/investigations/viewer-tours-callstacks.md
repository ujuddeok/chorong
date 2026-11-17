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
