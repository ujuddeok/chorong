- Built-declaration links now read tsconfig files with a comment after a trailing comma (`"rootDir":
  "src", // …`), which previously made the whole file unreadable. A pathological workspace glob in
  `package.json` or `pnpm-workspace.yaml` can no longer stall loading the index, and a `types` or
  `source` field that names a pattern or a declaration file is ignored instead of linked.
