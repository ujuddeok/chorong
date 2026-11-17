# Opening code in other editors

Chorong is read-only, so editing happens elsewhere. From any location Chorong can open the same
spot in another editor, or on the repository's web host.
Code: `crates/cho-core/src/editor.rs`.

## Editors

| Editor | Program | File at position | Project |
| --- | --- | --- | --- |
| Zed | `zed` | `zed PATH:LINE:COL` | `zed ROOT PATH:LINE:COL` |
| VS Code | `code` | `code --goto PATH:LINE:COL` | `code ROOT --goto …` |
| Cursor | `cursor` | `cursor --goto PATH:LINE:COL` | `cursor ROOT --goto …` |
| Sublime Text | `subl` | `subl PATH:LINE:COL` | `subl ROOT PATH:LINE:COL` |
| Xcode | `xed` | `xed --line LINE PATH` | |
| JetBrains (RustRover, WebStorm, PyCharm, IDEA) | `rustrover` etc. | `rustrover --line L --column C PATH` | |
| Custom | any | template with `{path}`, `{line}`, `{column}` | |

Zed, VS Code and Cursor also register URL schemes (`zed://file/…`, `vscode://file/…`,
`cursor://file/…`), used when the CLI isn't installed and in HTML exports.

## Web hosts

| Host | URL |
| --- | --- |
| GitHub | `https://github.com/O/N/blob/REV/PATH#L42` |
| Bitbucket Cloud | `https://bitbucket.org/W/N/src/REV/PATH#lines-42` |
| Bitbucket Data Center | `https://HOST/projects/P/repos/N/browse/PATH?at=REV#42` |

## Remote repositories

For a GitHub or Bitbucket repository Chorong reads from its own cached clone. "Open in editor"
needs a working tree you own, so the plan is: if you have a local clone registered for that
repository, open there at the same path; otherwise offer to open the web URL, or to clone.

## Using it

- Viewer: the status bar's **Open in Zed** button (choose the editor with
  `chorong-app --editor code`) opens the selected line, inside the folder you opened.
- CLI: `cho editor list`, `cho editor open FILE --line 42 --editor code [--dry-run]`,
  `cho web-url --repo github:o/n --rev SHA --path src/lib.rs --line 42`.
