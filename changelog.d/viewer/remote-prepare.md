- Open Remote has a **Prepare and index** checkbox (checked by default, remembered per project).
  Checked, the new window asks for the project's approval when it has none or the recipe or a
  build file changed (the panel lists the exact commands, the new or changed build files and
  where they run), runs the project's prepare steps in the background with progress and Cancel
  in the status bar, then builds the index. A failed step shows its output with Retry. Unchecked
  opens read-only, syntax only, and runs nothing. Esc in the approval panel opens without indexing.
- A failed prepare no longer stops indexing: the index is built anyway and reads `partial
  (prepare failed)`, and the failure (step, output tail, log path, **Retry**, **Dismiss**) is a
  notice that does not block the window. A successful Retry rebuilds the index cleanly, even if
  the partial build is still running. **Update** on a pull request asks again when the head
  moved: approval if the build files changed, prepare only if its last run no longer covers
  them, then indexing.
