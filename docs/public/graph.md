# Reference graph

Inbound and outbound references drawn as a graph: who uses a symbol, what it uses, and how
that rolls up into files and packages. It serves pull-request blast radius ("what could
this change affect?") and architecture reading ("what do I couple to if I use X?").
Code: `crates/cho-graph` (engine, JSON, DOT, HTML) and `crates/cho-cli/src/graph_cmd.rs`.

The graph is computed from the code; how it is drawn is a separate, replaceable front end.
The first front end is an HTML page exported by the CLI and the viewer. A native view
is planned, and only if the HTML proves the views are worth it.

## Facts: building the graph

- **Nodes** are declarations: SCIP symbols defined in the repository.
- **Edges** come from SCIP occurrences: each reference is attributed to the innermost
  declaration whose range contains it (the same machinery as `references_within` in
  [semantic.md](semantic.md)). Outbound edges are what a declaration references; inbound
  edges are who references it. One pass over the index builds both.
- **Edge kinds** come from Tree-sitter, by where the reference sits:

| Kind | TypeScript | Rust |
| --- | --- | --- |
| `calls` | callee of a call expression | callee of a call or method call |
| `constructs` | `new X` | struct literal, `X::new` stays `calls` |
| `renders` | JSX element `<X/>` | |
| `type-uses` | type annotation, `import type`, generic argument | type position, trait bound |
| `implements` | `extends`, `implements` (SCIP relationship) | `impl Trait for X` (SCIP relationship) |
| `re-exports` | `export { X } from` | `pub use` |
| `references` | anything else | anything else |

  Type-only edges are kept apart from runtime edges because that is where monorepo
  coupling hides.
- **Trust.** Every edge comes from the index, so edges are facts. What the index cannot
  see is stated, never filled in: dependency injection, event emitters, routing by string,
  calls on `any`, dynamic `import()` with a computed path. A Rust call through a trait
  points at the trait method; implementations are reached through `implements` and drawn as
  dispatch. Index status (`current`, `stale`, `partial`, `failed`) and the commit are part
  of every graph; a stale, partial or failed index also gets a note saying what it may miss.
- **No name matching.** An occurrence that resolves to no known symbol is an unresolved
  edge to a package or file, shown as such, never matched by name.

## Roll-up

Real graphs explode: a shared logger or `useQuery` has hundreds of users, and two hops is
unreadable. So the graph is always aggregated:

- **Levels:** symbol → file → package (pnpm workspace package, Cargo crate, Python
  package). Edges between rolled-up nodes carry counts ("@acme/web → @acme/core: 48
  references, 6 symbols") and expand on demand.
- **Repository only by default.** Third-party dependencies collapse to one node per
  package (`react`, `lodash`, `tokio`).
- **Caps.** Past a node limit per level, the rest become one "+212 more in @acme/web" node.
- **Cycles** (mutual recursion, packages that depend on each other) are kept, and drawn as
  visibly reversed edges.

## Views

- **Neighbourhood.** A symbol's inbound and outbound edges to a chosen depth.
- **PR blast radius.** Start from the changed declarations (diff hunks intersected with
  definitions in the head), walk inbound edges to a depth, roll up by package. Mark which
  test files are reachable: "these tests reference the changed code" is deterministic and
  labelled as reachability, not coverage. Indexing only the head shows who uses the new
  code; removed exports and removed edges need the merge base indexed too.
- **Outbound closure.** Everything X depends on, at package level: what you couple to if
  you use X. Chorong cannot simulate code that doesn't exist yet; this is the deterministic
  part of that question.
- **Architecture diff.** Package-level edges added and removed between merge base and
  head. Needs both revisions indexed; an index of the main branch can be cached and shared
  across pull requests, with the comparison labelled approximate when a pull request's
  merge base is older than the cached index.

## TypeScript monorepos

`scip-typescript` indexes one `tsconfig` project at a time, and Chorong merges the indexes.
A cross-package edge only resolves if both indexes name the symbol identically.

- **Packages that resolve to source** (Turborepo's "just-in-time" packages, `paths`, or
  `exports` pointing at `src`): names match and cross-package edges work.
- **Packages that resolve to build output** (`dist/*.d.ts`): the consumer's index names
  the `.d.ts` declaration while the defining package names the source. Chorong maps one to
  the other through declaration maps (`.d.ts.map`), which is deterministic. Without
  declaration maps, those edges are unresolved package edges, and Chorong says to enable
  `declarationMap`. Indexing such a repository also needs a build first.
- **Generated code** (Prisma, GraphQL or OpenAPI clients) exists only after install or
  build; until then, edges into it are unresolved.

Open: the first target is the owner's pnpm + Turborepo monorepo; whether its packages
resolve to source or `dist` is still to be checked.

Rust needs none of this: `rust-analyzer scip` indexes a whole Cargo workspace as one
index.

## Front ends

### 1. Self-contained HTML (first)

From the viewer and from the CLI alike; both call the same `cho-graph` code.

**In the viewer.** A **Show graph** command (keyboard shortcut, and a button in the context
sidebar) builds the graph in-process and opens the page in the default browser:

- On a symbol (under the cursor or selected in the sidebar): its neighbourhood.
- On a file or package (from the file tree or breadcrumbs): its inbound and outbound
  edges at package level.
- In a pull request or branch comparison: the blast radius of the change.

A small popover sets direction (inbound, outbound, both), depth and edge kinds, remembered
between uses. Pages are written to Chorong's cache directory and the path is shown, so a page
can be kept or shared. The viewer never waits on the graph: it is built in the background
and the page opens when ready.

As built (`app/src/graph.rs`, `app/src/view/graph_ui.rs`):

- **Entry points:** Cmd-K G and the palette's **Show Graph** open the popover; the context
  pane's **Show graph** button does too (for the hovered, pinned or caret symbol);
  right-click on a file in the sidebar, or a click on the file path above the source, opens
  it with that file chosen.
- **Start from:** the symbol at the focus (its exact SCIP symbol from the index, never a
  name), the file, the package of the file, and **Changes on this branch**: the blast radius
  from the merge base with `origin/HEAD` (else `main`, else `master`) to `HEAD`, offered when
  `HEAD` is not on that branch.
- **Options:** direction, depth (1 to 6 hops), edge kinds, and **for sharing** (the same as
  `--share`). Kept in `graph.json` in Chorong's settings.
- **Build:** `RefGraph::build` runs on the background executor and is kept until the index
  is reloaded. The page is written to `graphs/` beside the index cache, named after the
  start and the commit, opened with the default browser, and a note above the source shows
  its path with **Open again** and **Copy path**. Links in it name the repository by its
  remote and carry `from=graph`, so a click after the checkout has moved gets **Rebuild
  graph** in the mismatch banner.
- **Other hosts:** links recognise GitHub and Bitbucket only.
- Needs an index, describes the checkout: not available while viewing a file at another
  commit.

**From the CLI** (for scripts and agents):

```bash
cho graph --symbol "@acme/core:createSession" --in --out --depth 2 --html out.html [--open]
cho graph --diff BASE..HEAD --blast-radius --html out.html
cho graph … --json                 # the same graph as data, for agents
cho graph … --dot                  # for Graphviz and quick checks
```

- The Rust engine builds the graph; the page only draws it. The page holds the data, its
  script and the current Zed theme as CSS variables. ELK.js (layered layout with nested
  package and file boxes and orthogonal routes) and Cytoscape.js (pan, zoom, hover) are
  loaded from a shared copy in a `graph-assets/` folder (named by library versions) beside
  the pages in Chorong's cache folder, by a relative path that only goes down (Safari refuses a
  `file://` page's scripts from a parent folder), so a page is about 150 KB. A page for
  sharing (`--share`, or **For sharing** in the viewer) or `--inline` puts both inside the
  page (about 2 MB), and so does a page written anywhere outside Chorong's cache folder: Chorong
  adds no folders beside your own files, and no path into the cache ends up in the page. No network access either way: it works offline,
  and a shared page can be attached to a pull request. If the shared copy has gone, the
  page says so and how to get it back.
- In the page (decided with the owner after real graphs on zod and trigger.dev were
  unreadable; see "Readable graphs" in the design notes):
  - **Tests hidden** by default, shown in a blast radius; a toggle with a count either way.
  - **Only lines that lead to the focus** (one hop nearer it in the walk). Lines between
    nodes that are merely both in the picture are behind **other lines**.
  - **Disclosure:** past 10 neighbours on one side, the rest fold into "+N more" (leaves
    only, ordered by reference count then name). A click shows 10 more, the next opens a
    focused view of all of them, grouped by file when there are more than 30, with **Back**.
    It only reveals what the page holds; the engine's node cap still applies.
  - **Lines follow ELK's routes** (orthogonal, merged at shared ends), drawn as Cytoscape
    segments.
  - **Package level** draws each package as one closed box (the focus's package outlined).
    Line thickness is the reference count; the number is how many distinct things the line
    uses at its far end, on the line when the graph has more than 20 nodes, otherwise in
    the box ("N used").
  - **Side panel:** a line lists **Uses** and **From**; a symbol lists **Used by** and
    **Uses**; a package or file lists both grouped by the other package. Each row has its
    reference count and a thin muted bar, sorted by weight.
  - **Readable zoom:** a graph that does not fit at 0.8 zoom opens at that zoom centred on
    the focus, with an overview map (click or drag to move); **Fit** shows everything.
  - Hover lights up a node's edges; edge kinds can be hidden (hide `type-uses` to see
    runtime coupling).
- Every symbol node carries a `chorong://` link that opens the declaration in the viewer,
  where the reading loop takes over. Registering Chorong as the OS handler for `chorong://` ships
  with the graph (see [url-scheme.md](url-scheme.md)). Each node also offers the
  `cho open …` command, an editor link and the web URL, for machines where Chorong isn't
  installed as an app.
- Each file is a snapshot: expanding only uses data already in it, so going deeper means
  exporting again from that node. That friction is the main thing this stage measures.
- The `--json` output is what agents should build PR overlays and traces from, so their
  edges start out verified.

Layout is deterministic: ELK's layered algorithm with fixed options gives the same picture
for the same graph. Coordinates never come from an LLM.

## As built

- **Engine** (`cho-graph`): `RefGraph::build(&Semantic)` makes one pass over the index; views
  are pure functions: `neighbourhood`, `file_view`, `package_view`, `outbound_closure`,
  `blast_radius` (with `changed_decls` mapping diff hunks to declarations). Edge kinds come
  from `cho_syntax::classify_references`.
- **Declarations** are Tree-sitter outline entries that own a SCIP definition on their name;
  a reference is attributed to the innermost one. Code outside every declaration (imports,
  impl headers) is a **file-level** node. Its `references` are not drawn unless
  `--file-level`, because they restate the symbol edges; the page says how many were left out.
- **Honesty.** Engine edges are `verified` (the index saw them), except where a workspace
  package is used through its built declarations (`dist/*.d.ts`): those references are drawn to
  the source declaration they were mapped to (see
  [semantic.md](semantic.md#built-declarations-in-workspaces-dist-to-source)), the edge counts
  them as `mapped`, its status is `mapped` when all of its references came that way, and the
  page says so in a note and in the edge's details. Call sites that
  Tree-sitter sees but the index does not resolve are listed on their node as unresolved
  calls and drawn as no edge. References to a symbol with no definition in the index, in
  the source's own package (such as `Self` in impl blocks), are counted in a note and not
  drawn; in another package they become an unresolved package edge. When that package is
  in the repository, its unresolved node ("cho-core (unresolved)") sits inside the
  package's node, so the package is drawn once and its lines roll up into it.
- **Test files** are marked by path and name convention (`tests/`, `*_test.rs`, `*.spec.ts`,
  `mod tests`), labelled reachability, not coverage.
- **Blast radius** maps `git diff -U0 BASE..HEAD` hunks onto the indexed checkout. If the
  checkout is not at `HEAD`, or is dirty, the graph says so. Removed code needs the merge
  base indexed too and is only listed as deleted paths.
- **Links** in a page name the repository by its remote (`github:`, `bitbucket:`,
  `bitbucket-dc:`), or `local:~/…` when it has none Chorong recognises; `--repo` overrides.
  Each carries `from=graph`. The page header shows the worktree relative to where the
  command ran, or its folder name. `--share` drops the branch, dirty state and worktree
  name from links and the header (the page says "exported for sharing"); see "Repository
  identity in generated links" in [url-scheme.md](url-scheme.md).
- **Planned, not built:** the editor link on nodes, expanding a node beyond the data in the
  file, architecture diff, and declaration-map resolution for TypeScript.

## Graph pane (interim)

A small, static drawing of a graph inside the viewer, for tour pages and investigation
diagrams until a native graph view replaces it. It is separate from the HTML page, which is
unchanged.

- **Layout in Rust** (`cho-graph`, `diagram.rs`): a small layered layout written in Chorong, no
  dependency. Cycles are broken by a depth-first walk in input order, layers come from the
  longest path, long edges get bend points, crossings are reduced by barycentre sweeps, and
  edges are routed orthogonally. The edges leaving one node for the next layer share one
  trunk (a single line out, then a branch to each target), and so do the edges arriving at one
  node; arrowheads are only at targets. Each edge label sits on its own line, on the
  background colour, where it covers no box, no other label and no other line. Nodes with a group (a file, a
  package, or an investigation's group) are drawn inside a box; each group has its own column
  (row, left to right), so boxes never overlap. The same input always gives the same layout
  and byte-identical SVG: every order comes from the input, nothing is hashed or read from the
  environment, and text is measured with a fixed 12 px monospace advance.
- **Input**: a declaration's neighbourhood from the index (direction and depth), or an
  investigation's own nodes and edges. Edges are **verified** (solid; from the index),
  **claimed** (dashed; stated, or mapped through a built declaration) or **broken** (dotted, in
  the error colour). Nodes not in the code (third-party packages, investigation nodes with no
  declaration) have a dashed outline; the root is tinted with the accent colour.
- **In the viewer**: **In a pane** in the Show graph popover draws the selected declaration's
  neighbourhood in a graph pane (bottom bar), top down in a wide pane and left to right in a
  tall one, whichever Fit draws larger. **Fit** and **1:1** (Fit never goes below 85%, so text
  stays readable; past that the drawing scrolls from its top left, or from the focus when that
  would be out of view, and the toolbar says "scroll for more"), **Open in browser** (today's HTML page), and a click on a
  node opens its declaration. The layout runs once, in the background, under `graph.enabled`
  (about 1 ms for 60 nodes); drawing is static.
- **From the CLI**: `cho graph … --svg OUT` writes the same drawing as SVG with real text,
  12 px or larger, and the theme's colours as CSS variables (both themes by the reader's
  setting, or `--theme`). `--group file|package` boxes files or packages; `--top-down` runs
  edges downwards instead of left to right.

## NOTICE: third-party code in the HTML page

The page uses two unmodified libraries, pinned. Their licence texts are shipped next to the
bundles in `crates/cho-graph/assets/`. A self-contained page inlines both and repeats the
licence texts at the end; a normal page loads them from Chorong's shared copy, where the licence
texts sit beside them, and names both at its end.

| Library | Version | Licence | File |
| --- | --- | --- | --- |
| [Cytoscape.js](https://js.cytoscape.org) | 3.30.2 | MIT | `cytoscape.min.js`, `LICENSE-cytoscape-MIT.txt` |
| [ELK.js](https://github.com/kieler/elkjs) (Eclipse Layout Kernel, GWT-compiled) | 0.9.3 | EPL-2.0 | `elk.bundled.js`, `LICENSE-elkjs-EPL-2.0.md` |

EPL-2.0 is file-level copyleft: the ELK bundle stays unmodified and under its own licence;
its source is at the project linked above. Nothing is loaded from the network at runtime.

## Not in scope

- Simulating changes that haven't been written.
- Guessed edges of any kind, including name-based fallbacks when the index has no answer.
- Coordinates or layout from imported investigations.
