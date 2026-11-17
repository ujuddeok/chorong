# The `cho` CLI

A headless command line for people, scripts and agents. It never needs a display or GPU,
and every command takes `--json` for machine-readable output. Code: `crates/cho-cli`.

The CLI and the viewer run the same library code, so what the CLI reports is what the
viewer shows.

## Commands

| Command | What it does |
| --- | --- |
| `cho outline FILE` | declarations with qualified names, kinds, lines and byte ranges |
| `cho find FILE SYMBOL` | find a declaration (`Queue::push`, `Svc.run`); `unique`, `ambiguous` or `not-found` |
| `cho show FILE [--lines A:B]` | print lines with numbers |
| `cho highlight FILE` | highlight spans: byte ranges, grammar capture, Zed theme key (also JSON, TOML, XML and Markdown, which `outline` and `find` refuse) |
| `cho investigation validate FILE` | check the investigation format |
| `cho investigation check FILE --root DIR [--at worktree\|rev] [--no-index]` | resolve every anchor against a checkout (or, with `--at rev`, the investigation's pinned commit read from git); `FILE` may be an `https://` URL; edges the cached semantic index confirms are `verified`; format version 2 files also get their views, steps, external nodes, comparison rows and branches; exit 1 if any anchor is broken, including one in a view ([shape](#investigation-check-output)) |
| `cho investigation list [--root DIR]` | the investigations in `.chorong/investigations/`: title, kind and pinned commit, or why a file doesn't parse ([shape](#investigation-list-output)) |
| `cho index [--root DIR] [--only rust\|typescript\|python] [--force] [--timeout SECS]` | build or refresh the SCIP index (cached outside the repository); progress lines on stderr, Ctrl-C cancels and keeps the old index |
| `cho index status [--root DIR]` | `none`, `current`, `stale` or `failed`, per project |
| `cho definition FILE LINE:COL [--root DIR]` | where the symbol at a one-based position is defined; exit 1 if none |
| `cho references FILE LINE:COL [--root DIR]` | every occurrence of that symbol, definitions first. In a workspace whose packages export built declarations (`dist`), `definition`, `references` and `hover` also follow the link to source; such locations end with `[mapped (unverified): METHOD]` and carry a `mapped` object in `--json` ([semantic.md](semantic.md#built-declarations-in-workspaces-dist-to-source)) |
| `cho hover FILE LINE:COL [--root DIR] [--peek]` | what the index records about that symbol: name, signature, documentation, definition, index commit and status; `--peek` adds each definition's full declaration; exit 1 if no symbol ([shape](#hover-output)) |
| `cho symbols QUERY [--root DIR] [--limit N]` | search symbol definitions by display name |
| `cho search PATTERN [--root DIR] [--regex] [--case-sensitive] [--glob G]... [--limit N]` | search file contents with ripgrep's engine; literal and case-insensitive by default; respects `.gitignore`; `--glob '*.rs'` includes, `--glob '!*.md'` excludes; at most `--limit` (default 5,000) matching lines; exit 1 if nothing matches |
| `cho changes (--base REV \| --merge-base TARGET) [--head REV \| --worktree] [--root DIR]` | the files and declarations that differ between two commits, read from the object store (the checkout is read only with `--worktree`, which compares with the working folder and marks uncommitted work): each changed declaration is `added`, `changed`, `removed` or `ambiguous`; `--merge-base` makes Before the merge base of `--head` (default `HEAD`) and TARGET ([shape](#changes-output)) |
| `cho graph (--symbol Q [--closure] \| --file F \| --package P \| --diff BASE..HEAD --blast-radius \| --pr REPO N) [--in] [--out] [--depth N] [--kinds K,..] [--max-nodes N] [--externals] [--file-level] [--repo REF] [--share] [--root DIR] [--dot \| --html OUT [--inline] [--open]] [--svg OUT [--group none\|file\|package] [--top-down]] [--theme dark\|light]` | reference graph from the cached index; `--json` prints it as data, `--dot` Graphviz, `--html` a page (in Chorong's cache folder it loads its libraries from a copy beside it; anywhere else, or with `--inline` or `--share`, everything is inside the page), `--svg` a static diagram laid out by Chorong (the drawing of the viewer's graph pane; `--group` boxes files or packages) ([graph.md](graph.md)). Links name the repository by its remote (`--repo` overrides); `--share` drops branch, dirty state and worktree name. `--pr REPO N` opens the pull request as `cho open` does and shows its blast radius (merge base..head) from the worktree's index |
| `cho settings [--repo DIR]` | the effective settings, each with its source (`default`, `global`, `repo`, `lightweight`) ([shape](#cho-settings)) |
| `cho settings get KEY [--repo DIR]` | one effective value (text: the value alone) |
| `cho settings set KEY VALUE [--repo DIR]` / `cho settings unset KEY [--repo DIR]` | write or remove a value in `settings.json`, or in the repository's override with `--repo`; never touches the repository |
| `cho link parse URL` (also prints the provisional `pr`, `base`, `side`) / `cho link build --repo … [--path --symbol --line --rev --branch --dirty]` | `chorong://` links |
| `cho editor list` / `cho editor open FILE [--line --column --editor --project --dry-run]` | other editors |
| `cho web-url --repo … --rev … --path … [--line]` | GitHub / Bitbucket URL for a file |
| `cho theme FILE` | validate a Zed theme and list its themes |
| `cho open TARGET... [--name NAME] [--compare REF] [--dry-run]` | open a file, folder or `chorong://` link in `chorong-app`; several files open together (a focus layout, the rest in the tray), and `-` reads paths from standard input; `--name` titles the window (always a new one) so it can be told apart; `--compare REF` opens it comparing the whole repository with that branch, tag or commit, as Compare Against… does (always a new window; `--view` is accepted and changes nothing); `cho TARGET...` is shorthand ([below](#cho-path)) |
| `cho open REPO (--branch B \| --pr N \| --rev SHA) [--base REV] [--prepare] [--view]` / `cho open URL [--base REV] [--prepare] [--view]` | open a remote branch, pull request or commit: fetch it into Chorong's cache, place a worktree, print it with Before (the merge base with the target branch, or `--base`), After and how far behind the target it is; `--prepare` then runs the approved prepare recipe and indexes; `--view` opens the worktree in the viewer ([sources.md](sources.md)) |
| `cho project list` / `cho project add REPO [--protocol ssh\|https] [--url URL] [--worktree-cap N]` / `cho project remove REPO` | remote repositories Chorong remembers; `remove` also clears their cache |
| `cho pr REPO N` | a pull request's number, title, author, state, branches and head, from the host |
| `cho prepare show\|approve REPO [TARGET]` / `cho prepare set REPO (-- ARGV… \| --file F)` / `cho prepare run REPO [TARGET] [--force]` / `cho prepare clear REPO` | the commands a remote worktree needs before indexing, their approval (covers prepare and indexing) and running them ([sources.md](sources.md#prepare)) |
| `cho auth add HOST [--email E]` / `cho auth status [HOST] [--no-check]` / `cho auth remove HOST` | host tokens in the OS keychain; `add` reads the token from standard input; `status` never prints it |
| `cho cache list` / `cho cache clear [REPO [WORKTREE]]` | disk use of the repository cache; clear everything, one repository or one worktree (with their indexes) |
| `cho formats` | list the formats agents can write for Chorong |
| `cho schema investigation` | print the investigation JSON Schema |
| `cho skill investigation [--install DIR] [--force]` | print or install an agent skill for writing investigations |

`cho callers` (from SCIP) and `cho serve` (the same commands over stdio or MCP) are planned.

## `cho PATH`

`cho <folder-or-file>`, with no subcommand, is shorthand for `cho open PATH`, like `zed .` and
`code .`: `cho .` opens the current folder in the running viewer, or starts one.
`cho a.rs b.rs c.rs` opens the files together: one window rooted at the repository holding
them, the first file on a focus page and the rest in the tray. `cho -` reads the paths from
standard input (`git diff --name-only | cho -`). `--name "PR 412 review"` puts that name first
in the window's title (`PR 412 review — cho — Chorong`, as the Window menu and Mission Control
show it), so a window an agent opened is easy to find among others; a named window is always a
new one. Without a name, files opened together are titled `cho — 3 files — Chorong`.

- A subcommand name always wins. A folder called `search` is reached as `cho ./search` (or
  `cho open search`); only a first argument that is not a subcommand and exists on disk is
  treated as a path. Anything else gets the usual "unrecognized subcommand" error.
- `--json` and `--dry-run` work as for `cho open`.
- `cho PATH --compare main` (or `chorong-app PATH --compare main`) opens straight into a
  comparison with `main`: Before is the merge base of HEAD and `main`, as with Compare
  Against… in the viewer. It works for a local folder or file; for a remote branch or pull
  request use `--base` with `--branch`, `--pr` or `--rev`.
- On Linux there is one viewer per user: `chorong-app PATH` (and so `cho PATH`) hands the path to
  the running viewer over its local socket instead of starting another, as `chorong://` links do
  ([url-scheme.md](url-scheme.md)). A folder that is already open comes forward; otherwise a
  new window opens.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | the command ran but found a problem or no result: invalid file, broken anchors, symbol not found, no definition, a failed indexer |
| 2 | usage or I/O error (with `--json`: `{"error": "…"}` on stdout) |

Errors are one short line naming the host or repository and the cause (`Can't reach github.com
(connection refused)`). For remote sources, `--json` adds `kind` (`offline`, `auth`,
`not-found`, `ambiguous`, `lock-busy`, `unsupported`, `cancelled` or `other`) and `detail`
(the git command and its output); in text, `--verbose` (`-v`) prints the detail. Neither ever
contains a token or URL credentials.

## Changes output

`cho changes` compares two commits of a local repository without touching the checkout: the
changed files come from a tree compare in the object store, changed lines from an in-process
line diff, and each side's changed lines are mapped onto that side's Tree-sitter declarations.
It is the changed-symbol list of a pull request when Before is the merge base
(`--merge-base main`). Text output marks files `A`, `M`, `D`, `R` and declarations `+` added,
`~` changed, `-` removed and `?` ambiguous. `cho --json changes` returns (stable shape,
`version` 1; key order is not significant):

```json
{
  "version": 1,
  "base": "3f8cc5f…",
  "head": "288bc96…",
  "merge_base": { "target": "main", "candidates": ["3f8cc5f…"] },
  "notes": [],
  "files": [{
    "status": "modified",
    "path": "src/queue.rs",
    "kind": "file",
    "language": "Rust",
    "level": "symbol",
    "outside_declarations": false,
    "coarse": false,
    "hunks": [{ "base": { "start_line": 7, "line_count": 1, "bytes": [120, 148] },
                "head": { "start_line": 7, "line_count": 1, "bytes": [120, 152] } }],
    "symbols": [{
      "qualified_name": "Queue::push",
      "kind": "method",
      "status": "changed",
      "base": [{ "range": [80, 156], "name_range": [87, 91], "start_line": 6, "end_line": 8, "touched": true }],
      "head": [{ "range": [80, 160], "name_range": [87, 91], "start_line": 6, "end_line": 8, "touched": true }]
    }]
  }]
}
```

- `--worktree` makes After the working folder instead of `--head`: `head` is the checked-out
  commit, `worktree` is `true`, and files are the base-to-HEAD changes plus files that differ
  between HEAD and the working folder (edited, staged, deleted and untracked ones, which count
  as `added`; gitignored files are skipped). Nothing is run or written. Each hunk and each
  declaration then carries `"uncommitted": true` when HEAD to the working folder touches it,
  and each file that differs from HEAD carries it too. A hunk that committed and uncommitted
  work both touch is cut at the edge of the uncommitted part, so every hunk is one or the other;
  a line changed by both counts as uncommitted. The field is additive at `version` 1 and
  omitted when false, so a missing `uncommitted` means `false`, as does a missing `worktree`.
- `base` and `head` are full commit ids. `merge_base` is present only with `--merge-base`:
  `candidates` lists every merge base (sorted); with several (criss-cross merges), `base` is the
  most recently committed, ties broken by the smallest id, and `notes` says so.
- `files` are sorted by path. `status` is `added`, `modified`, `deleted` or `renamed`; `path` is
  the head path (the base path for a deleted file). Renames carry `old_path` and `similarity`
  (100 for identical contents, else the share of Before's bytes kept, as git scores it);
  pairs below 50 are a deletion and an addition, as with git's defaults. With more than
  10,000 added × deleted candidate pairs only identical-content renames are found, and
  `notes` says so. A mode change alone (`chmod +x`) is `modified` with no hunks.
- `kind` is `file`, `symlink` or `submodule`. `level` is `symbol` when declarations were
  compared, else `file`, with `reason`: `unsupported-language` (also JSON, TOML, XML and
  Markdown, which have no declarations), `binary` (a NUL byte in the first 8,000 bytes),
  `not-utf8`, `symlink`, `submodule`, `too-large` (a working file over the 16 MB source
  limit) or `unreadable` (a working file Chorong can't read inside the folder, such
  as a symbolic link out of it); those last two have no `hunks`. Unsupported languages and
  non-UTF-8 text still have `hunks`.
- `hunks`: `base` lines were replaced by `head` lines. Lines are one-based; `line_count` 0 is a
  pure insertion or deletion, positioned before `start_line`. `bytes` are UTF-8 byte offsets
  (end exclusive). Pure insertions and deletions slide down past repeated lines as git's do. A
  diff costing more than 2,000 line edits is reported as one block between the common prefix
  and suffix, with `coarse: true`.
- `symbols`: each changed declaration once, matched across sides by qualified name and kind
  only. `added`: not at base; `removed`: not at head; `changed`: one on each side;
  `ambiguous`: several declarations share the name and kind on a side (overloads, two `impl`
  blocks with the same method), so all are listed and none is picked. `base` and `head` list
  every declaration with that name and kind on that side, in source order; `touched` says the
  side's changed lines fall inside it. A declaration counts as touched when changed
  non-whitespace text is inside it but outside its nested declarations, so adding a method does
  not change its class. Comment, doc-comment, attribute and decorator lines directly above a
  declaration count as part of it. Changed text outside every declaration (imports, top-level
  statements) sets `outside_declarations`.
- A declaration's identity on a side is (that side's path, `qualified_name`, `kind`); for a
  rename the base path is `old_path`. The viewer keys its Before/After switch the same way.
- A declaration moved between files is `removed` from one and `added` to the other; Chorong does
  not pair them. Callers of removed declarations are not listed (Before is not indexed).
- Unknown revisions and unrelated histories are errors (exit 2). Code: `crates/cho-changes`,
  `crates/cho-git`.

## Search output

`cho --json search` returns `{pattern, regex, case_sensitive, truncated, matches}`. Each match
is `{path, line, line_start, text, ranges}`: `path` is relative to `--root`, `line` is one-based,
`line_start` is the byte offset of the line in the file and `ranges` are `{start, end}` byte ranges of every
match within `text`. Results come in path order. Code: `crates/cho-search`.

## Hover output

`cho --json hover FILE LINE:COL` returns (stable shape):

```json
{
  "found": true,
  "symbol": "rust-analyzer cargo proj 0.1.0 util/helper().",
  "local": false,
  "name": "helper",
  "local_text": "helper",
  "display_name": "helper",
  "kind": "Function",
  "range": [47, 53],
  "signature": { "text": "pub fn helper() -> u32", "language": "rust", "source": "signature-documentation" },
  "no_signature": null,
  "documentation": ["Markdown from doc comments."],
  "definitions": [{ "path": "src/util.rs", "line": 1, "column": 8, "range": [7, 13], "is_definition": true, "text": "pub fn helper() -> u32 {" }],
  "index": { "commit": "4f1c…", "status": "current" },
  "mapped": null
}
```

- `mapped` is set when the symbol is a workspace package's built declaration joined to its
  source: `{ "symbol": SOURCE_SYMBOL, "method": "declaration-map" | "tsconfig" | "source-field" |
  "path-swap", "through_reexports": BOOL, "verified": false, "label": "mapped (unverified): …" }`.
  The signature, documentation and definitions are then the source symbol's, and each definition
  carries the same `mapped` object. In `definition` and `references` output, a location's
  `mapped.symbol` is the symbol that occurrence belongs to.

- `range` is the byte range of the occurrence under the position.
- `name` is the symbol's canonical name: the index's `display_name`, else the last descriptor
  of the symbol ID (`…/money.ts/add().` gives `add`), else, for locals (`local N` has no name),
  the source text. `local_text` is the source text the occurrence covers; it differs from `name`
  when code renames (`import { add as plus }` gives `name` `add`, `local_text` `plus`).
  `display_name` is a deprecated copy of `name`.
- `signature` is the indexer's own text or `null`; it is never synthesised. `source` is
  `signature-documentation` or `documentation` (scip-typescript puts it in the first
  documentation entry). When it is `null`, `no_signature` is `no-symbol-information` (only
  occurrences, e.g. std and dependencies) or `not-recorded`.
- `kind` is the SCIP kind, empty when the index has no symbol information.
- `definitions` is empty for symbols defined outside the index.
- `index.status` is `current`, `stale` or `failed`; `index.commit` is the commit the index was
  built at, or `null` without Git.
- With `--peek`, every entry of `definitions` also has `declaration_range` and `no_declaration`:

  ```json
  "declaration_range": { "kind": "declaration", "start": 0, "end": 32, "start_line": 1, "end_line": 3,
                         "marked_range": null },
  "no_declaration": null
  ```

  `start` and `end` are the shown declaration's byte range in the defining file (signature and
  body); the lines are one-based and inclusive. `kind` says which case it is:
  - `declaration`: the symbol's own declaration, the innermost declaration in the file's
    Tree-sitter outline whose name contains the definition. `marked_range` is `null`.
  - `enclosing`: the definition is a field, property, parameter or local, which has no outline
    entry, so the range is the innermost outline declaration that contains it (the struct,
    class or interface, or the function or method). `marked_range` is the member's line(s):
    `{ "start", "end", "start_line", "end_line" }`, a byte range inside the declaration (cut to
    the declaration's start and end, so it can begin mid-line) with one-based lines.

  When nothing in the outline contains the definition (e.g. a top-level `let` in a script),
  `declaration_range` is `null` and `no_declaration` is `outside-indexed-files` (file missing,
  unreadable or outside the root), `no-declaration` or `unsupported-language`. Without
  `--peek` neither key is present. In text mode `--peek` prints the declaration's lines under
  each `defined at`; for `enclosing`, the label says so and every line is prefixed, `› ` on
  the marked ones and two spaces on the rest.
- No symbol at the position: `{"found": false, "reason": "…", "index": {…}}`, exit 1.

## Investigation check output

`cho investigation check` reports each node (`resolved`, `ambiguous` or `broken`, with byte
`range`, `region`, `name_range` and one-based `line`) and each edge (`verified`, `claimed` or
`broken`), plus:

- `at`: `worktree` or `rev`, the code that was checked.
- `drift`: how the checkout relates to the investigation's `rev`:
  `{"state": "at-rev", "head": "…"}`, `{"state": "differs", "head": "…", "rev_present": true}`
  (`head` is `null` in a repository with no commits) or
  `{"state": "not-git-repo", "reason": "…"}`.
- `source`: `{"path": "…"}`, or `{"url": "…", "host": "…", "final_host": "…"}` for a fetched
  file (`host` is the one the URL names, `final_host` where it was fetched from after
  redirects).
- `index`: whether the semantic index was used. With `--at rev` it is used only when the
  checkout is at `rev`, because it describes the working tree.

A format version 2 file also gets these keys (each left out when empty; a version 1
file's output is unchanged):

- `nodes[].author_evidence`: the file's `read` or `inferred`, as written. It sits beside
  `status` and Chorong never changes it.
- `steps` (`tour`, `callstack`): `index`, `id`, `number` (`S1`, `S2`… for setup steps,
  `1`, `2`… for the rest), `phase`, `name`, `external`, `author_evidence`, `values`, `depth`
  (`{"n": 2, "verification": "claimed"}`, from `calls` and `dispatches` edges from the first
  step; `verified` only along verified edges) and `called_by` (`[{"id", "rel",
  "verification"}]`).
- `views`: each written view, resolved: `step`, `arrangement`, `default` (true when the
  step falls back to the default page), `fallback` (why) and `panes`. Each pane has its
  `kind`. An `editor` pane has `path` and `highlights` (`number`, `note`, `anchor`). A
  `context` or `outline` pane has an `anchor`. A `graph` pane has `node`, `external`,
  `anchor`, `direction` and `depth`. A resolved anchor has `node`, `label`, `path`, `status`,
  `range`, `region`, `name_range`, `line` and `messages`.
- `externals`: `id`, `what`, `label`, `status` (always `not-in-code`) and `author_evidence`.
- `untraced`: `what`, `why` and the resolved `at` anchor.
- `comparison`: `sides` (`id`, `label`, `path`, `variants`) and `rows`. Each row has `id`,
  `name`, `number`, `phase`, `cells` (one per side: `side`, `step` or `missing`, `external`,
  the resolved `anchor`, `values`, `note`), `result`, `result_source` (`derived`, `claimed`
  or `none`), `same_code`, `callout`, `note` and `messages`. The `variants` key lists each
  variant's rows, resolved with that variant chosen.
- `branches` (tour): `id`, `label` and the `overrides`, each with `step`, `node`, `reached`
  and `missing`.
- `messages`: what check reports about these parts, such as an unknown pane kind, a
  highlight in another file, or a row result that conflicts with the code.

`--at rev` reads each anchored file with git, never from the working tree; if the pinned
commit isn't in the repository, the command fails (exit 2) and says so. A URL must be
`https://` without credentials; the fetch is limited to 4 MB and 15 seconds, sends no
cookies or credentials, and follows at most 5 redirects, all to `https://`. Files are limited
to 4 MB too.

## Investigation list output

`{"root": "…", "entries": [ … ], "truncated": false}`, sorted by file name. Each entry has
`path` (repository-relative) and either `title`, `kind`, `rev`, `nodes` and `steps`, or
`reason` when the file can't be read or isn't a valid investigation. Only regular `.json`
files directly in the folder are listed; a symbolic link that leads outside the worktree is
listed with a reason, not read. At most 500 files are listed (`truncated` says if there were
more). The command exits 0 even when some files are invalid; `check` them for details.

## For agents

Chorong describes its own input formats, so an agent doesn't need this repository to write them:

```bash
cho formats                                         # what can I write?
cho schema investigation                            # exact JSON Schema
cho skill investigation --install .claude/skills    # or .cursor/skills
```

The skill is generated from the same build as the CLI (format version and wording included),
so it always matches what `cho investigation check` accepts. An installed copy is a file in
the repository and does not update itself: it carries a version stamp (a line after its front
matter), and `cho investigation check` and `cho investigation list` say on stderr, and as
`skill_outdated` in `--json`, when a copy under `.claude/skills` or `.cursor/skills` is not
this cho's. Run the install again with `--force` to update it. It writes format version 2
(`cho formats` gives `version`, what the skill writes, and `versions`, every version Chorong
reads), and teaches:

- choosing the kind: `callstack` to trace one path, `tour` to read code in order (including
  several files side by side with highlighted chunks), `comparison` for paths that do the same
  job, `diagram` for an unordered picture;
- views (panes and arrangements, never sizes), highlights and `from`/`to` ranges, external
  nodes, phases, step names, summary points, the untraced list, author evidence and runtime
  values, comparison results, variants and branches;
- written and derived: get declaration names, uses, callers and branch changes from Chorong
  (`find`, `outline`, `references`, `graph`, `changes`) instead of grepping, write only the
  judgement, save the file in `.chorong/investigations/`, and run `cho investigation check`
  before handing it over.

Every JSON example in the skill is checked by the test suite against a fixture. This
repository keeps a copy at `.claude/skills/chorong-investigations/`; regenerate it with `--force`
when the format changes.

- Always pass `--json`; text output is for people and may change.
- Positions are reported as one-based `line` plus zero-based UTF-8 byte `range`s.
- Use `cho find` and `cho outline` to name declarations exactly when writing
  investigations, then `cho investigation check` before handing a file over.

## `cho settings`

Reads and writes Chorong's own config directory (`$XDG_CONFIG_HOME/chorong`, else `~/.config/chorong`;
`~/Library/Application Support/chorong` on macOS; `CHORONG_CONFIG_DIR` replaces it), never a
repository. What each setting means, and the file formats: [settings.md](settings.md).

- `cho settings` prints one line per setting (`key  value  source`), then warnings. Exit 1 if
  a settings file has a syntax error (the last good values still apply and are printed).
- `set` validates the key, the type and the range before writing, edits only that key in the
  file (comments and unknown keys stay), and refuses to touch a file that has a syntax error.
  Booleans are `true`/`false`; numbers are whole numbers within the setting's range. Exit 2 on
  any refusal.
- `--repo DIR` (a folder that exists) resolves for that repository, and with `set`/`unset`
  writes its override, keyed by canonical path, in `repos.json`.
- `set` reports the *effective* result: if Lightweight mode is on, a saved `true` still reads
  `false (lightweight)`.

`cho --json settings` (stable shape):

```json
{
  "config_dir": "/home/me/.config/chorong",
  "files": {
    "settings": { "path": ".../settings.json", "status": "ok", "error": null },
    "repos": { "path": ".../repos.json", "status": "missing", "error": null }
  },
  "repo": "/canonical/path/or/null",
  "lightweight": false,
  "version": 0,
  "settings": [
    {
      "key": "changes.auto_refresh", "group": "changes",
      "value": true, "source": "default", "default": true,
      "global": null, "repo": null,
      "lightweight_overrode": null,
      "explanation": "true from default",
      "kind": { "type": "bool" },
      "description": "Refresh the Changes list when the window is activated",
      "cost_hint": "Heavy, in the background: …",
      "cost_class": "background-heavy",
      "lightweight_off": true
    }
  ],
  "unknown_keys": [{ "file": "settings.json", "kind": "unknown-key", "key": "future.key", "repo": null, "message": "…" }],
  "diagnostics": []
}
```

- `status` is `ok`, `missing`, `malformed` or `unreadable` (`error` then says why).
- `source` is `default`, `global`, `repo` or `lightweight`. When it is `lightweight`,
  `lightweight_overrode` is `{"would_be": …, "from": "repo"}`.
- `kind` is `{"type": "bool"}` or `{"type": "uint", "min": …, "max": …}`; `cost_class` is
  `background-heavy`, `per-pointer`, `per-frame`, `on-demand` or `negligible`.
- `diagnostics` holds every problem: `kind` is `malformed`, `unreadable`, `unknown-key` or
  `invalid-value`. `unknown_keys` is the unknown-key subset.
- `get`, `set` and `unset` print one `settings` entry; `set`/`unset` add `scope`
  (`global` or `repo`), `file` and `changed`.
