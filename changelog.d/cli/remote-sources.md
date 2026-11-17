- Open GitHub and Bitbucket branches, pull requests and commits without a clone of your own:
  `cho open github:acme/api --pr 123` (or a URL) fetches into Chorong's cache, places a worktree
  and prints Before (the merge base with the target branch, or `--base`), After and how far
  behind the target it is. New `cho project`, `cho pr`, `cho auth` and `cho cache`
  commands; `cho graph --pr REPO N` shows a pull request's blast radius. Tokens live in the OS
  keychain, with environment variables as the fallback.
