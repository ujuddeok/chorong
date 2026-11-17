- Opening a folder no longer runs programs named in its git config. A folder unpacked from an
  archive could set `core.fsmonitor`, a filter or diff driver, or a hook, and Chorong's read-only
  `git status` would run it before any trust prompt. Chorong now switches those off for every git
  command it runs. Submodules no longer count towards the "dirty" state.
- On Linux without `XDG_RUNTIME_DIR`, the viewer refuses a link socket folder
  (`/tmp/chorong-UID`) that another user owns or can enter.
