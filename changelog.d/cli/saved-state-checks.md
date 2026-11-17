- A hand-edited `projects.json` is checked on load as strictly as `cho project` checks new
  entries (clone URLs, recipes, worktree cap, auto-prepare approval), naming the bad entry.
  Repeated folders in the recent list are shown once, and unsafe worktree names in a
  repository's cached state are dropped.
