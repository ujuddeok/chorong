- Settings has three new sections, **Projects**, **Cache** and **Accounts**. Projects edits how
  each remote project is cloned (HTTPS or SSH), how many worktrees it keeps and automatic
  prepare (available once the recipe is approved), shows its approval, and removes a project
  after saying what goes. Cache shows the disk use of every clone and worktree (measured when
  you open it) and clears one worktree, one project or everything after a confirmation. Accounts
  shows each host's token source, checks a token on request and adds or removes tokens in the
  keychain from a masked field; without a keychain it names the environment variable to set.
