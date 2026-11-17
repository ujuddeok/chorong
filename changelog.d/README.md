# Changelog fragments

Each user-facing change adds **one new file** here instead of editing `CHANGELOG.md`, so
parallel branches never edit the same lines.

- Path: `changelog.d/<section>/<short-name>.md`, e.g. `changelog.d/viewer/pane-pickers.md`.
- Sections: `settings`, `viewer`, `graph`, `semantics`, `cli`, `investigations` (a new folder
  name becomes a new heading).
- Content: one or more Markdown bullets (`- ...`), wrapped at 100 columns, written for users.

`scripts/changelog.py` assembles them: `scripts/changelog.py` previews the Unreleased section;
`scripts/changelog.py --release VERSION` writes it into `CHANGELOG.md` under that version and
deletes the fragments (run it before tagging a release).
