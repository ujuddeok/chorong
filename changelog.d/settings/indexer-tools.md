- New settings: `index.typescript_version` and `index.python_version` (an exact version, `^`/`~`
  range or `latest`; a newer indexer needs no new Chorong), `index.version_check` and
  `index.version_check_days` (how often a range or `latest` is checked, weekly by default; off in
  Lightweight mode), `index.typescript_install` and `index.python_install` (your own install;
  Chorong then downloads nothing), and `tools.node`, `tools.npm`, `tools.uv` (filled the first time
  Chorong finds them).
