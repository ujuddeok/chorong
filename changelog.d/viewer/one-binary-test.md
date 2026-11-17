- The viewer and the Changes list agree on which files are binary (a NUL byte in the first
  8,000 bytes, as git decides): a file whose first NUL comes a little later now opens as text.
