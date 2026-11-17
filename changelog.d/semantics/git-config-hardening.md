- Repository configuration can no longer make Chorong's git commands run a program through a filter
  or diff driver whose name has capital letters or an `=` (such as `[filter "Evil"]`). More
  program-running settings are switched off for repositories Chorong reads (`core.gitProxy`,
  `remote.<name>.uploadpack`, merge drivers, `pager.<command>` and others).
- Two indexing runs of the same project at once (two Chorong windows) no longer make one of them
  fail, and index status writes no longer collide.
