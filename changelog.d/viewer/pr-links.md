- Copy chorong:// Link in a pull-request view (or a Before/After comparison) now includes the pull
  request number, its head, the comparison's Before and the side you are reading
  (`pr`, `rev`, `base`, `side`; provisional). Opening such a link starts the same comparison at
  your place. A link to a pull request Chorong has not fetched asks first ("Open pull request #123
  of github:acme/api?") and fetches only if you choose Open. A Before that is not available, a
  pull request that moved on and a version lost to a force-push each say so.
- The question before fetching a pull request from a link also offers "Open your worktree at
  this commit" when a folder of yours for that repository is at the link's commit: it opens
  there with the same comparison and fetches nothing.
