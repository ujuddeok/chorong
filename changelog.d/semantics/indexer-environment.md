- Indexers no longer inherit your whole environment. Build scripts and npm tooling see only
  what a build needs (paths, toolchain, compiler, proxy and locale settings), so tokens and keys
  exported in your shell, such as `GITHUB_TOKEN`, `AWS_*` or `NPM_TOKEN`, stay out of their
  reach. The trust prompt now says plainly that trusted code runs as you.
