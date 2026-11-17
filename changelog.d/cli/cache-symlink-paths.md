- Opening a remote repository or pull request no longer fails with "refusing to write outside
  Chorong's cache" when the cache's own path goes through a symbolic link (for example a linked
  home or projects folder, or macOS temporary folders under `/var`).
