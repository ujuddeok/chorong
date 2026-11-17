- A pull request whose details the host couldn't give (and that had no saved copy) now shows in
  the header band as that pull request, not as a commit. Worktrees remember what they were
  opened for as data, so recents and links read it back even if the wording changes; worktrees
  opened by earlier versions still read as before.
- A `chorong://` link whose Before commit no longer exists opens with the merge base again when
  the link spells the commit in upper case.
