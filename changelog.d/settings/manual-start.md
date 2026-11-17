- `index.auto_build` and `prepare.auto` are retired: a settings file that still has them keeps
  them and says they were retired. `index.auto_refresh` now means "keep the index fresh in every
  session" and is off by default (and in Lightweight mode). New: `index.low_priority` (on by
  default) runs indexers below the viewer's priority.
- Open Remote's **Prepare and index** box is unchecked by default and never remembered; a
  project's auto-prepare is ignored (dropped from `projects.json` on the next save) and the
  Settings Projects switch is gone.
