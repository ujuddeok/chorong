- TypeScript indexing no longer runs out of memory on large monorepos: Chorong gives scip-typescript
  an 8 GB Node heap by default (the `index.node_memory_mb` setting; a `NODE_OPTIONS` heap size wins). Before, an installed trigger.dev
  checkout died with "JavaScript heap out of memory".
