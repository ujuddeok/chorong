- Graphs open in Safari again: a page in Chorong's cache folder loads its libraries from a copy
  beside it rather than from a parent folder, which Safari refuses for local files. A page
  written outside the cache folder (`--html OUT`) now has the libraries inside it. Build an
  older graph again to fix it.
