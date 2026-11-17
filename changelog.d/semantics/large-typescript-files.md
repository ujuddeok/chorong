- TypeScript files over 1 MB are now indexed (up to 8 MB). scip-typescript skipped them silently,
  so a large generated parser or a one-file type checker had no definitions or references.
