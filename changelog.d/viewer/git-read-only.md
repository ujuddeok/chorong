- Reading git state (branch, dirty flag, index freshness) no longer refreshes the repository's
  index: Chorong's `git status` calls run with `GIT_OPTIONAL_LOCKS=0`, so `.git/index` is never
  rewritten and `index.lock` is never taken.
