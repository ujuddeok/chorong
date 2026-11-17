- Remote-source errors are one short line naming the host or repository and the cause, such as
  "Can't reach github.com (connection refused)". `--verbose` shows the git command and its
  output, and `--json` adds the error's `kind` and `detail`. Neither shows tokens or URL
  credentials.
