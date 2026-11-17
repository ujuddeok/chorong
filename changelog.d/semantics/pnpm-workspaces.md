- TypeScript indexing now works in pnpm workspaces that have no root `tsconfig.json` (most
  Turborepo monorepos): Chorong passes `--pnpm-workspaces` to scip-typescript when it finds
  `pnpm-workspace.yaml`. Before, the index failed with "missing tsconfig.json".
