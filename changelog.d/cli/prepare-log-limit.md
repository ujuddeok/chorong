- A prepare step that prints without end is stopped once its log passes 64 MB, instead of filling
  the disk until its timeout; the folders you opened recently are no longer forgotten because one
  entry in `recent.json` was malformed.
