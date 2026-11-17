# Semantic navigation

Go-to-definition, references and symbol search come from **SCIP indexes** produced by batch
indexers, never from a running language server and never from guessing by name.
Code: `crates/cho-semantic`.

## Indexers

| Language | Detected by | Command |
| --- | --- | --- |
| Rust | `Cargo.toml` | `rust-analyzer scip ROOT --output OUT` (install: `rustup component add rust-analyzer`) |
| TypeScript / JavaScript | `tsconfig.json` or `package.json` | `node SCIP_TYPESCRIPT index --max-file-byte-size 8mb --output OUT [PROJECTS…]`: Chorong's own copy of scip-typescript 0.6.0 ([below](#the-indexers-chorong-runs)), run in the project folder. Needs the project's `node_modules`. With a `pnpm-workspace.yaml`, PROJECTS are the root and each workspace package, which Chorong lists itself. The Node heap comes from `index.node_memory_mb` unless `NODE_OPTIONS` sets one |
| Python | `pyproject.toml`, `setup.py`, `setup.cfg` or `requirements.txt` | `node SCIP_PYTHON index --project-name NAME --output OUT`: Chorong's copy of scip-python 0.6.8, with `PYTHONSAFEPATH=1`, and the project's `.venv` first on `PATH` when it has one (Python 3.10+) |
| Java | | none yet; syntax only |
| Swift | | none yet; syntax only |

Indexers run as subprocesses with a timeout, never on the UI thread, never writing into the
repository. A repo with several projects gets one index per detected project root, merged at
load time.

## The indexers Chorong runs

scip-typescript and scip-python are Node programs. Chorong installs its own copy of each, once,
into `tools/` in its index cache (`…/chorong/indexes/tools/scip-typescript/0.6.0/`), and runs it
as `node` with the copy's script by absolute path; the project folder stays the working
directory. The install is `npm install --ignore-scripts` in a folder of Chorong's own, moved into
place when complete. So nothing in a repository chooses the program: not a package of the same
name in its `node_modules`, not its `.npmrc`, and not its package manager (scip-typescript's
`--pnpm-workspaces` would run `pnpm` in the repository; Chorong reads `pnpm-workspace.yaml`
itself and passes the packages, root first, as `pnpm ls -r` lists them).

- **Versions are settings:** `index.typescript_version` (default 0.6.0) and
  `index.python_version` (default 0.6.8) take an exact version, a range (`^0.6.0`, `~0.6.0`)
  or `latest`, so a newer indexer needs no new Chorong. A range or `latest` is checked against
  the registry (`npm view`) when a build starts and the last check is older than
  `index.version_check_days` (default 7); a newer match is installed then. `index.version_check`
  off (and Lightweight mode) keeps the installed match. Each index's `meta.json` records the
  version that made it.
- **Your own install instead:** `index.typescript_install` or `index.python_install` names a
  package folder or its script; Chorong then downloads nothing.
- **Where `node` and `npm` are:** the `tools.node` and `tools.npm` settings. Empty at first; the
  first build that needs them looks in your login shell's `PATH` (`$SHELL -l -c env`, at most 5
  s), then `/opt/homebrew/bin`, `/usr/local/bin`, nvm, volta and fnm installs, `~/.local/bin`
  and `/usr/bin`, and saves what it found. It never looks in the repository, and passes over a
  version manager's shim (asdf, mise, volta's `bin`), which would pick Node from the
  repository's files. Not found: the build fails with every place searched and the setting to
  set. This is what lets Chorong started from Finder or the Dock, with macOS's minimal `PATH`,
  index TypeScript and Python.
- **Python:** scip-python starts `python3` in the project. Chorong sets `PYTHONSAFEPATH=1`, so that
  Python imports nothing from the project folder. When the project (or the repository root)
  has a `.venv`, as `uv sync` makes, its `bin` goes first on `PATH`, so imports from installed
  packages resolve. Starting that Python runs its packages' `.pth` files, which is project
  code: TypeScript indexing runs none, Python with a `.venv` and Rust (build scripts,
  procedural macros) do.

## Indexing from the viewer

The viewer builds the index itself, so a reader never has to leave it, and reading never
waits for an index. (Shipped; the notes below are what it does.)

- **Build index** is an action in the status bar (`Index: none` becomes a button), in the
  context pane's "needs an index" message, and in the command palette.
- It runs in the background, one project root at a time, with progress in the status bar.
  When an index finishes it is loaded in place: the context pane and exact highlighting
  start working without reopening the folder.
- **Trust, once per repository.** Indexers run the project's own code: Rust build scripts
  and procedural macros. The first build
  in a repository asks whether to trust it; trusting means letting the project's code run as
  you, and it can read your files.
  The answer is remembered per repository (by canonical path, with its git remote; a
  different remote at that path asks again) in Chorong's own settings, never
  in the repository. Chorong never runs a package install in your folders; if a TypeScript
  project's `node_modules` is missing, the banner warns that imports between packages won't
  resolve until you install, and `cho index` names the command to run. (Remote projects opened into Chorong's own
  cache can [prepare](sources.md#prepare) their worktrees, after approval.)
- **Indexers get a minimal environment.** They see the variables a build needs (`PATH`,
  `HOME`, toolchain, compiler, proxy and locale settings) and none of the rest, so tokens and
  keys exported in your shell (`GITHUB_TOKEN`, `AWS_*`, `NPM_TOKEN`, `SSH_AUTH_SOCK`, ...) are
  not handed to build scripts. Files such as `~/.npmrc` are still readable, so a private
  registry keeps working. If a build needs another variable, set it in a config file the
  tool reads (`.cargo/config.toml`, `.npmrc`).
- **Opening a folder runs nothing from it.** Before trust, Chorong only reads: it runs read-only
  `git` commands (status, branch, remote), and a repository's own git config cannot make
  them run programs. Fsmonitor, filter and diff drivers it defines are switched off, and so are
  hooks and submodules, so a folder unpacked from an archive is safe to open.
- **Every build starts with your click.** Opening a folder, trusted or not, never starts a
  build: files read and highlight at once, and a banner says what a build would run (the
  indexers, whether the project's own code runs, the last build's duration) with a **Build
  index** button (for an untrusted folder the same click asks for trust). A `stale` index
  keeps answering, labelled as stale; the banner offers **Refresh** and **Keep fresh this
  session**, which refreshes a stale index without asking until the window closes. The
  `index.auto_refresh` setting does that in every session (off by default and in
  Lightweight mode). Indexers run below the viewer's priority unless `index.low_priority` is
  off.
- A failed build shows `Index: failed` with the stored stderr tail one click away.

## Cache

`$XDG_CACHE_HOME/chorong/indexes/<repo-id>/` (macOS: `~/Library/Caches/chorong/indexes/<repo-id>/`),
where `<repo-id>` is a hash of the canonical repository path. `CHORONG_CACHE_DIR`, if set, replaces
the whole `.../chorong/indexes` directory (tests, CI). Each index has a `.scip` file
and a `meta.json`: indexer name and version, project root, Git `HEAD`, a hash of
`git status --porcelain`, creation time, exit status, and a stderr tail.

Status reported to users:

| Status | Meaning |
| --- | --- |
| `none` | no index yet |
| `current` | `HEAD` and working-tree hash match |
| `stale` | they don't; the index is still used, and results are labelled as possibly stale |
| `failed` | the last run failed; see the stored stderr tail |
| `partial` | built after a [prepare](sources.md#prepare) step that failed (remote worktrees only); used, but parts that needed the prepared files may be missing; `meta.json` names the prepare log |

## Positions

SCIP ranges are `[line, startCol, endLine?, endCol]` in the document's position encoding
(UTF-8, UTF-16 or UTF-32 code units; unspecified means UTF-16 in practice). They are
converted to byte ranges only through `SourceBuffer`, against the file as currently read.
An occurrence whose range doesn't fit the current bytes is dropped, never clamped into a
wrong place. Multi-line occurrences (rust-analyzer emits one per module) never answer
`symbol_at`.

## Queries

- `symbol_at(path, byte)`: the SCIP symbol whose occurrence covers the byte.
- `definitions(symbol)`, `references(symbol)`: locations, definitions first. Built declarations
  of workspace packages are joined to their source (see
  [below](#built-declarations-in-workspaces-dist-to-source)).
- `search(query)`: case-insensitive substring match on symbol display names, definitions only.
- `references_within(symbol, path, byte_range)`: whether `symbol` is referenced inside a
  range. This is what verifies investigation edges.

`symbol_at` runs on every caret move in the viewer, so it does not scan a document. When an
index is built, each document gets a table of its single-line occurrences sorted by (line, start
column); `symbol_at` binary-searches the line and looks only at occurrences that start at or
before the position on it. Cost: one `Vec<(u32, u32, u32)>` entry (12 bytes) per named
single-line occurrence, built in `ScipIndex::new` and `merge` (a sort per document), and
O(log n + k) per query for k occurrences on the line. Results are identical to a full scan,
including ties (the earlier document and occurrence win). `occurrences_in(symbol, path)` returns a
symbol's byte ranges in one file by walking only that symbol's occurrences; the viewer uses it
for exact highlight groups.

Local symbols (`local N`) are per-document and never matched across files.

## Built declarations in workspaces (`dist` to source)

In a pnpm, npm or yarn workspace whose packages export their build (`package.json` `types` or
`exports` pointing at `dist/*.d.ts`), TypeScript resolves an import from another workspace
package to the built declaration. scip-typescript then gives every such use the symbol of the
`.d.ts` file (`… npm @acme/core 1.0.0 dist/`thing.d.ts`/Thing#`), which has no definition in
the index, while the declaration in `src` has a different symbol with only its own package's
references. Chorong joins the two when it loads the index:

1. **Workspace packages** come from `pnpm-workspace.yaml` `packages` or `package.json`
   `workspaces` at the repository root (or an indexed project's root), matched against the
   directories that hold indexed documents. For each package Chorong reads its name, version,
   `types`, `typings`, `source` and `exports` (including a source export condition such as
   `@acme/source`), and tsconfig `outDir`, `declarationDir` and `rootDir` (`tsconfig.build.json`,
   else `tsconfig.json`, following relative `extends`).
2. **Candidates** are symbols whose package name and version equal a workspace package's, whose
   file is a declaration file (`.d.ts`, `.d.mts`, `.d.cts`) under that package's output directory,
   and that have no definition in the index. Packages outside the workspace (`node_modules`
   dependencies) are never joined.
3. **A source file is proposed**, strongest first: the **declaration map** (`thing.d.ts.map`
   `sources`); **tsconfig** `outDir`/`declarationDir` to `rootDir`; the manifest's **source field**
   or source export condition, for an entry point; a **path swap** from the output directory to
   `src` (`dist/a/b.d.ts` to `src/a/b.ts` or `.tsx`).
4. **The link is checked**: it is made only if the index has a definition of the symbol with the
   same package, version and descriptor chain (`Thing#method().`) in the proposed file. A name
   matching in some other file never counts. If two different definitions match, nothing is
   linked.
5. **Bundled declarations** (tsup, tsdown and similar emit one `index.d.ts` with no maps): when
   no proposed file defines the symbol, Chorong follows the proposed source entry's
   `export * from` and `export { name } from` statements, relative ones only, to the file that
   defines the same descriptor chain. A renamed re-export (`export { a as b }`) is not followed,
   since its descriptors can't be checked. These links say `through re-exports`.

Not linked: members of anonymous type literals (`Options#typeLiteral12:maxTokens.`).
scip-typescript numbers anonymous type literals per file, so the built declaration and the
source give the same member different descriptors and the link can't be checked. On vercel/ai
every named declaration of the two built packages was linked and every unlinked candidate was
such a member.

Every link is labelled **mapped (unverified)** with its method, for example
`mapped (unverified): declaration map` or `mapped (unverified): path swap, through re-exports`.
The index did not record it, so it is never shown as verified.

What uses the links:

- `definitions` of a built-declaration symbol with no definition of its own returns the source
  definition, marked `mapped`.
- `references` from either symbol returns the union, definitions first; each location that
  belongs to the other symbol carries `mapped` with that symbol (`mapped.symbol`).
- `occurrences_in` (the viewer's highlights) includes both symbols.
- Hover on a built-declaration symbol shows the source symbol's signature and documentation,
  with `mapped`.
- The graph draws references to a built declaration to the source declaration's node; an edge
  whose references all came this way has status `mapped`, and every edge counts its mapped
  references (`mapped`).
- `references_within`, which verifies investigation edges, counts only a symbol's own
  occurrences: a mapped link verifies nothing.

Cost: computed once per index load and kept with it. It reads the workspace manifests and
tsconfigs, one declaration map per candidate file and the source entries a bundled declaration
leads to, and makes one pass over the index's symbols. Measured on vercel/ai (108 workspace
packages, two of them built, 5,727 candidates, 680 links; 163 MB index): 0.18 to 0.20 s of a
2.5 s index load in a release build, 0.8 s of 6 s in a debug build. A repository without a
workspace file pays only the directory scan of its documents.

## Hover information (types and signatures)

Chorong runs no language server, so types come from what the indexer recorded. SCIP symbol
information carries hover-style text: `signature_documentation` (a code snippet such as a
function signature or a variable's type, with its language) and `documentation` (markdown
doc comments). Indexers differ in what they fill in, especially for locals; measure it per
indexer and record the findings here rather than assuming.

What the indexers record (measured on this repository and on
`docs/public/guide/samples/monorepo/`; `scip` crate 0.10, whose `signature_documentation` is a
`Signature { text, language, occurrences }`):

| | rust-analyzer 1.97.0 | scip-typescript 0.6.0 |
| --- | --- | --- |
| `signature_documentation` | every symbol with symbol information, language `rust` | never set |
| Signature text found in | `signature_documentation` | `documentation[0]`, one fenced ` ```ts ` block |
| `documentation` | doc comments only (about a third of items have them) | the signature block, then doc comments |
| `display_name` | set | always empty |
| Functions, methods | `pub fn new(index: ScipIndex, …) -> Self` | `function add(a: Money, b: Money): Money`, `(method) …` |
| Types | `pub struct Location` (no fields or body) | `interface Money`, `class OrderService`, `type Currency` (no body) |
| Fields, properties | `pub range: Range<usize>` | `(property) cents: number` |
| Constants | `const HELPER: &str` (no value) | `var TAX_RATE: 0.2` (literal type) |
| Parameters | `root: impl Into<PathBuf>`, `self: &Semantic`; closure parameters with inferred types (`h: u64`) | `(parameter) items: Money[]`; arrow parameters inferred (`l: OrderLine`) |
| Locals, annotated | `let mut best: Option<Candidate>` | `var reserved: OrderLine[]` |
| Locals, inferred | `let safe: bool`, `let buf: Arc<SourceBuffer>` | `var subtotal: Money`, `var order: Order \| undefined` (`const` shown as `var`) |
| Symbols from std and dependencies | no symbol information (`external_symbols` empty): 38% of occurrences here | none either: 19% of occurrences in the sample (`Map`, `Math.round`, `Promise`) |

So both indexers already give the type of every local, inferred ones included; what neither
gives is anything for symbols defined outside the indexed project.

- `hover_at(path, byte) -> Option<Hover>` (built on `symbol_at`) and
  `hover(symbol) -> Option<Hover>`. `Hover` holds the symbol, its display name and kind, the
  signature (text, language and source) if the index has one, the documentation, where it is
  defined, the hovered occurrence's byte range, and the index's commit and status. The commit
  and status are those of the project whose root contains the file; with several projects
  that disagree and no file to tie the answer to, the commit is `null`.
- `signature_documentation` is kept on load (merging projects fills fields one index lacks).
  When it is empty and the first `documentation` entry is exactly one fenced code block, that
  block is the signature (source `documentation`) and is removed from the documentation; this
  is how scip-typescript records it. Nothing else is read as a signature.
- No type is ever synthesised. Without a signature, `no_signature` says why:
  `no-symbol-information` (the index only has occurrences, e.g. std and dependencies) or
  `not-recorded` (it has information but no signature).
- **Names.** `Hover.name` is the symbol's canonical name: the index's `display_name` or, when
  that is empty, the last descriptor of the SCIP symbol ID, parsed with the `scip` crate's
  grammar (`…/src/money.ts/add().` gives `add`; escaped names, methods, type parameters and
  every descriptor suffix are handled). Locals (`local N`) have no name in the ID, so they use
  the source text. `Hover.local_text` is the source text the hovered occurrence covers; it
  differs from `name` when code renames on import (`import { add as plus }`), and the viewer
  can then show `plus → add`. `Hover.display_name` was replaced by `name` (only the CLI used
  it; the CLI JSON keeps `display_name` as a deprecated copy).
- **Definition peek.** Each `Hover.definitions` entry is a location plus `declaration_range`
  (the full byte range of a declaration in the defining file, from the Tree-sitter outline, so
  the viewer can show a struct's fields, a constant's value or a whole signature),
  `declaration_kind` and `marked_range`. Two cases:
  - `declaration`: the definition is the name of an outline declaration (the innermost one
    whose name range contains it). `marked_range` is `None`.
  - `enclosing`: fields, properties, parameters and locals have no outline entry. When the
    definition lies inside an outline declaration, the range is the innermost such
    declaration (a field's struct, class or interface; a parameter's or local's function) and
    `marked_range` is the member's line(s): the lines the definition occurrence covers,
    cut to the declaration's range. The marking is by line and is never a guess about
    structure; a multi-line member marks only its first line.
  - When nothing in the outline contains the definition (a top-level `let` in a script),
    `no_declaration` says why: `outside-indexed-files`, `no-declaration` or
    `unsupported-language`.
  Reading the lines is the viewer's job, through `SourceBuffer`, so they are always the file
  as it is now.
  - It is computed **on request only**: `Semantic::peek_definition(location)` (and `declaration_range`, its range only),
    `Semantic::peek(&mut hover)` and `Semantic::hover_at_with_peek(path, byte)`. Plain
    `hover_at` never parses.
  - A miss parses the defining file (about 0.3 ms for a small file; more for large ones), so
    call it off the UI thread and only when a card or shortcut asks. Outlines are cached per
    file, keyed by path, file length and modification time, in an LRU of 64 files; a hit costs
    a `stat` plus a lookup (about 2 µs, measured in a debug build for an `enclosing` peek,
    which also reads the cached file buffer for the marked lines).
- Answered from the in-memory tables: one `symbol_at` plus the symbol's definitions.

The Critical Lens classifies each call token with `symbol_at`. A **project package** is any
`scheme manager name` (the first three fields of a SCIP symbol) that has a definition
occurrence in the index. A symbol in a project package, or a local one, is a **repo** call; any
other global symbol is **external**; a token with no symbol is **unknown**. Nothing is
inferred from names.

## Verifying investigations

`cho-investigation` defines an `EdgeOracle` trait; `cho-semantic` implements it. A
`calls`, `references` or `dispatches` edge becomes **verified** when the index has a
reference to the symbol defined at the `to` anchor inside the `from` anchor's declaration
range. Without an index, or when the index can't answer, the edge stays **claimed**.

## CLI

```bash
cho index [--root DIR] [--only rust|typescript|python] [--json]   # build or refresh
cho index status [--root DIR]
cho definition FILE LINE:COL            # one-based line and column (UTF-8 bytes)
cho references FILE LINE:COL
cho hover FILE LINE:COL [--json]        # signature, docs, definition, index commit
cho symbols QUERY [--root DIR]
cho investigation check FILE --root DIR [--no-index]   # uses the cached index when present
```
