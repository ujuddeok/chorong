- A failed indexer's report keeps Node's reason (`FATAL ERROR: …`) instead of losing it behind a
  long native stack trace, so the out-of-memory hint naming `index.node_memory_mb` shows again.
  `cho index`, `cho index status` and the viewer lead with the reason.
