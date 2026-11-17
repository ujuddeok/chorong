- Indexing runs at a lower CPU priority (`nice -n 10`, with everything it starts), so opening
  and reading files stays responsive while the first index builds.
