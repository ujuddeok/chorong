- `cho a.rs b.rs …` (and `cho open`, `chorong-app`) opens several files in one window rooted at
  their repository: the first on a focus page, the rest in the tray. `cho -` reads the paths
  from standard input, so `git diff --name-only | cho -` opens what you just changed.
