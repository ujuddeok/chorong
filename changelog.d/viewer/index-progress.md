- A running index build now shows where it is and for how long: `Index: stale · indexing
  packages/core (14 of 38) · 2m 10s`. Elapsed time is always shown, even when the indexer prints
  no steps. A large TypeScript workspace gets a one-line note up front: it may take minutes and GBs
  of memory.
- Index builds can be cancelled: **Cancel Indexing** in the command palette, or click the
  status-bar item (or press Cmd-K I again) and confirm. The indexer and its child processes are
  stopped, and the previous index stays in place, shown as `Index: stale · cancelled`.
