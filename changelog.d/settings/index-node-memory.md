- New setting `index.node_memory_mb` (default 8192): the memory limit for the TypeScript
  indexer's Node process. A `NODE_OPTIONS` with its own `--max-old-space-size` still wins, and a
  "JavaScript heap out of memory" failure now names the setting.
