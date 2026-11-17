- The TypeScript and Python indexers run from Chorong's own copies, installed once into Chorong's cache
  (`npm install --ignore-scripts`) and started with `node`, instead of `npx` in the repository: a
  package of the same name in the repository's `node_modules`, its `.npmrc`, or its package
  manager no longer chooses the program. For pnpm workspaces Chorong lists the packages from
  `pnpm-workspace.yaml` itself instead of running `pnpm ls` in the repository (same index).
- scip-python runs with `PYTHONSAFEPATH=1`, so Python imports nothing from the project folder; a
  project's `.venv` (as `uv sync` makes it) goes first on `PATH`, so its packages' imports
  resolve.
- Chorong started from Finder or the Dock can index TypeScript and Python: `node`, `npm` and `uv`
  are found in the login shell's `PATH` and the usual install places (Homebrew, nvm, volta, fnm,
  `~/.local/bin`, `~/.cargo/bin`) and saved as settings. Not found: the error lists every place
  searched.
