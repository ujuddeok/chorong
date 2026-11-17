- In pnpm, npm and yarn workspaces whose packages export built declarations (`dist/*.d.ts`),
  go to definition, references, hover, the context pane and the reference graph now reach the
  source declaration from other packages. Chorong maps a built declaration to its source by the
  declaration map, tsconfig `outDir`/`rootDir`, the manifest's `source` field or source export
  condition, or a `dist` to `src` path swap (bundled declarations through the source entry's
  re-exports), and links only when the index has a definition with the same name chain in that
  file. Such links are labelled "mapped (unverified)" with their method.
