- `cho changes --worktree` compares the base (`--base REV` or `--merge-base TARGET`) with the working
  folder instead of a commit: edited, staged, deleted and untracked files count, gitignored ones do
  not. Each hunk and declaration touched by uncommitted work carries `"uncommitted": true`
  (additive at version 1; absent means false).
