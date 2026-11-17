- `cho index` no longer says a TypeScript package "has no node_modules" when they are hoisted
  to the repository root, or when the package declares no dependencies (the viewer already
  accepted both).
- A `.chorong/prepare.json` that is a folder is ignored with a note, like an invalid one, instead of
  failing prepare and indexing.
- Git failures that merely mention a pull request numbered 401 or 403 are no longer taken for
  an authentication failure.
- `cho graph --pr REPO 0` is refused as not a pull-request number.
