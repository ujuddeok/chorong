# `dist` TypeScript workspace fixture

A small pnpm workspace whose packages export built declarations (`dist`), as most
monorepos do. `app` imports every other package, so scip-typescript gives its uses the
symbols of the built `.d.ts` files. The tests check that Chorong joins
each one to its source, and only when the source definition can be checked.

| Package | Built declarations | How Chorong finds the source |
| --- | --- | --- |
| `@fx/maps` | `dist/shapes.d.ts` with a declaration map; source in `lib/geometry.ts` | declaration map |
| `@fx/tscfg` | `out/calc.d.ts`, tsconfig `rootDir: code`, `outDir: out` | tsconfig |
| `@fx/entry` | `types/index.d.ts`, manifest `source: ./main/entry.ts` | source field |
| `@fx/swap` | `dist/greet.d.ts`, nothing else | path swap; `Missing` is declared only in `dist`, so it is not mapped |
| `@fx/bundle` | one bundled `dist/index.d.ts` | path swap through the source entry's re-exports; the renamed export is not mapped |

`index.scip` was generated once, outside the tests, with Chorong's pinned scip-typescript:

```sh
pnpm install --offline   # links the workspace packages; nothing is downloaded
npx -y @sourcegraph/scip-typescript@0.6.0 index --output index.scip
```

The `node_modules` links are not committed. The index's metadata `project_root` was then set to
`file:///fixture/dist-workspace` so it names no machine path; Chorong does not read that field.

Used by `crates/cho-semantic/tests/dist_links.rs`, the graph, CLI (`crates/cho-cli/tests/ts_workspaces.rs`)
and context-pane tests, the `ts-*` verify checks, and the `scip_index` fuzz seeds, through
`cho_fixtures::TsWorkspace::Dist` (see `fixtures/README.md`).
