- Trust now follows the repository, not just the folder: Chorong records the trusted folder's git
  remote, and if a different project later sits at the same path, it asks again before running
  anything, saying which remote changed. Folders trusted by older versions keep their trust.
