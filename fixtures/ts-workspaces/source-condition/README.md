# `source-condition` TypeScript workspace fixture

The other common TypeScript monorepo shape (see `docs/typescript-workspaces.md`): packages
are found through a custom export condition that points at source, and command-line `tsc`
reads the per-file declarations that project references emit into `.types/`. `dist/` holds
bundled JavaScript only.

| Package | Shape |
| --- | --- |
| `@fx/typescript-config` | shared `base.json` (`customConditions: ["development"]`, NodeNext) and `library.json` (`composite`, `declaration`, `declarationMap`, `emitDeclarationOnly`) |
| `@fx/core` | `exports` `.`: `development` → `./src/index.ts`, `default` → `./dist/index.mjs`; tsconfig extends `@fx/typescript-config/library.json` (a package specifier), `outDir: .types`, `rootDir: .`; `.types/src/*.d.ts` with declaration maps |
| `@fx/client` | the same, depending on and referencing `core` |
| `@fx/ui` | subpath exports straight to source (`./components/*`, `./lib/*`), no conditions; `.types/` without declaration maps (`rootDir: src`) |
| `@fx/web` | an app importing all three, with `references` to each |

The root `tsconfig.json` is a solution file (`files: []`, references only).

The `.types/` and `dist/` folders are committed as a build would leave them (`tsc -b`, minus
`*.tsbuildinfo`), so the fixture is the *built* checkout. `index.scip` was generated once,
outside the tests, with Chorong's pinned scip-typescript, the way `cho index` runs it:

```sh
pnpm install --offline     # links the workspace packages; nothing is downloaded
rm -rf packages/*/.types   # clean checkout
npx -y @sourcegraph/scip-typescript@0.6.0 index --pnpm-workspaces --output clean.scip
tsc -b tsconfig.json       # built checkout: writes packages/*/.types
npx -y @sourcegraph/scip-typescript@0.6.0 index --pnpm-workspaces --output built.scip
cmp clean.scip built.scip  # identical with 0.6.0
```

Both indexes were byte-identical: scip-typescript resolves `@fx/core` through the
`development` condition to `src/*.ts` and, unlike command-line `tsc`, does not redirect to the
referenced project's `.types/*.d.ts`. If a future scip-typescript does redirect, the clean and
built indexes will differ; regenerate both and check
`crates/cho-semantic/tests/source_condition.rs`, which also
covers what the dist→source join would do with `.types` symbols.

The index's metadata `project_root` was set to `file:///fixture/source-condition-workspace` so
it names no machine path; Chorong does not read that field. `node_modules` is not committed.

Used by `crates/cho-semantic/tests/source_condition.rs`, the CLI
(`crates/cho-cli/tests/ts_workspaces.rs`) and context-pane tests, the `ts-*` verify checks, and
the `scip_index` fuzz seeds, through `cho_fixtures::TsWorkspace::SourceCondition` (see
`fixtures/README.md`).
