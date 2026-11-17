- `cho investigation list` lists the investigations in a repository's
  `.chorong/investigations/` folder, with title, kind and pinned commit, or why a file is invalid.
- `cho investigation check --at rev` checks anchors against the investigation's pinned
  commit, read from git, instead of the working tree; check output now says how the checkout
  relates to that commit (`drift`).
- `cho investigation check` accepts an `https://` URL (4 MB limit, 15-second timeout, no
  credentials, redirects only to `https://`).
