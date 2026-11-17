- `cho changes --base REV | --merge-base TARGET [--head REV]` lists the files and declarations
  that differ between two commits (added, changed, removed, or ambiguous when a name has
  several declarations), read from the object store without touching the checkout. `--json`
  gives a stable shape with byte ranges on both sides.
