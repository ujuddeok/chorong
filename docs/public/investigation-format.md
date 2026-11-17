# Investigation format

An **investigation** is a JSON file of claims about code: a call stack, a reading tour, a
diagram, or an overlay on a pull request. LLMs and agent skills write them; Chorong checks them
against the code and draws them. Chorong never generates one itself.

- Schema: [investigation.schema.json](../../crates/cho-cli/schema/investigation.schema.json)
- Examples: [examples/](examples/) (each is checked by the test suite)
- Code: `crates/cho-investigation`

## Shape

```jsonc
{
  "chorong": 1,                       // format version: 1, or 2 (see "Version 2" below)
  "title": "How a push webhook reaches the job queue",
  "kind": "callstack",             // callstack | tour | diagram | pr-overlay | comparison (v2)
  "repo": "github:acme/api",       // same syntax as chorong:// links
  "rev": "3f9c1e2a7b4d",           // full or abbreviated commit hash, never a branch
  "summary": "…",                  // optional markdown
  "generator": "callstack-skill 0.1",
  "nodes": [ { "id": "handle", "anchor": { … }, "label": "…", "note": "…" } ],
  "edges": [ { "from": "route", "to": "handle", "rel": "dispatches", "note": "…" } ],
  "steps": ["route", "handle"],    // reading order; required for callstack and tour
  "groups": [ { "label": "HTTP layer", "members": ["route", "handle"] } ],
  "pr": { "number": 482, "base": "…", "head": "…" }   // pr-overlay only; head == rev
}
```

Unknown fields are rejected, so typos fail loudly rather than being ignored. Version 2 fields
are rejected in a version 1 file.

## Anchors

```json
{ "path": "src/jobs/queue.rs", "symbol": "Queue::push", "excerpt": "self.tx.send(job)", "line": 42 }
```

- `path`: repository-relative.
- `symbol`: a declaration, qualified the way the language writes it. `Queue::push` and
  `Queue.push` are equivalent. Leave it out to anchor a whole file.
- `excerpt`: optional exact text inside the declaration. Narrows the anchor to a precise
  spot, and detects drift: if the excerpt is gone, the anchor is broken.
- `line`: optional one-based hint, used only without a `symbol`. **Prefer symbols**: line
  numbers go stale with every edit, declarations rarely do.

In version 2 an anchor can give a range by text instead of one `excerpt`:

```json
{ "path": "src/http/webhook.rs", "symbol": "WebhookHandler::handle", "from": "verify_signature(", "to": "?;" }
```

`from` is searched inside the declaration (or the file), and `to` after the end of `from`.
The range runs from the start of `from` to the end of `to`. Both ends are checked, and a
missing end makes the anchor broken. Use `from`/`to` or `excerpt`, not both. There are never
line numbers.

## Relations

| `rel` | Meaning |
| --- | --- |
| `calls` | `from` calls `to` directly |
| `dispatches` | indirect: callback, trait object, route registration, event dispatch |
| `references` | `from` mentions `to` without calling it |
| `implements` | `from` implements or overrides `to` |
| `data-flow` | data produced in `from` reaches `to` |
| `contains` | module, type or file containment |
| `related` | anything else; say what in `note` |

## Verification

Chorong labels every node and edge. Labels come from Chorong, never from the file.

| Label | Node | Edge |
| --- | --- | --- |
| **verified** | (not used for nodes) | a semantic index confirms the relationship |
| **claimed** | (nodes are *resolved* or *ambiguous*) | both ends resolve but the index can't confirm it |
| **broken** | file, symbol or excerpt missing at `rev` | either end is broken |

Today, without a semantic index, the best an edge can be is **claimed**. Once SCIP lands,
`calls` and `references` edges become **verified** when a reference to `to` lies inside
`from`'s declaration.

Check a file from the command line:

```bash
cho investigation check docs/public/examples/callstack-webhook.json --root path/to/checkout
```

## Version 2

`"chorong": 2` adds the fields below. Chorong reads both versions, and the agent skill
(`cho skill investigation`) writes version 2. Examples:
[tour-views.json](examples/tour-views.json) (a tour with views, highlights, phases, a branch),
[comparison-rest-soap.json](examples/comparison-rest-soap.json) (a comparison with a variant),
[callstack-external.json](examples/callstack-external.json) (a call stack with external
nodes), [tour-actions.json](examples/tour-actions.json) (a tour with a diff pane and an action
step).

Every part is either **written** (the author's claim, shown as claimed at best) or
**derived** (worked out by Chorong from the code and the index). Chorong never upgrades a written
claim.

### Steps: names, evidence, runtime values

```jsonc
{ "id": "handle", "anchor": { … },
  "name": "Verify, then dispatch",   // short step name, apart from `label` and `note`
  "evidence": "read",                // read | inferred: how the author knows
  "values": ["event = \"push\""] }   // runtime values as the author saw them
```

- `evidence` is shown beside Chorong's own label (`resolved`, `verified`…), never in place of
  it.
- `values` are always the author's claim (`flag = false`, `refused`, `not reached`) and are
  never verified.

### Summary bullets and what could not be traced

```jsonc
"points": ["Signatures are checked before parsing."],
"untraced": [ { "what": "Handlers registered by plugins", "at": "handle", "why": "Registered by name at run time." } ]
```

`at` is optional: a node id or an anchor, checked like any other.

### External nodes

```jsonc
"externals": [ { "id": "db", "what": "database", "label": "Orders database", "evidence": "inferred" } ]
```

External nodes are things on the path that aren't code. `what` is `model`, `database`,
`service`, `vendor`, `platform`, `limit` or `other`.

- They have no anchor and share node ids, so steps, edges, groups and comparison rows can
  name them.
- They are always reported as `not-in-code` and never verified. An edge to or from one is
  `claimed` at best.

### Action steps

```jsonc
"actions": [ { "id": "migrate", "name": "Run the migration",
               "note": "Apply the migration to your local database before reading on.",
               "commands": [ { "label": "Apply", "text": "cargo run -p db -- migrate up" },
                             { "text": "psql -c '\\d orders'" } ] } ]
```

An action step is an instruction for the reader, not code: a short `name`, a `note` (inert
Markdown) and optional `commands`, each a `text` with an optional `label`.

- Only `tour` files have actions. They share the node id namespace, so `steps`, groups and
  `views` can name them, and so can a tour branch's `step`.
- They are never part of the graph: edges, graph panes, editor anchors, comparison rows and
  the untraced list can't name them. Check reports them as `action`, never verified or
  broken, and they have no depth or callers.
- **Chorong never runs a command.** The viewer shows each one as text with a Copy button, and
  Copy puts exactly the `text` on the clipboard.
- An action's view adds panes beside its card, for "run this, then watch this code". Its
  editor panes need an `anchor`, since the action has no code of its own.
- Files that use `actions` need a Chorong that knows them: every release does, and so does any
  build from commit `0a0f5d3` on. An older development build rejects the whole file, because
  unknown fields are rejected. A `diff` pane needs no
  newer Chorong to open: an older build treats it as an unknown pane kind and shows the default
  page.

### Phases

```jsonc
"groups": [ { "label": "At start-up", "members": ["config"], "phase": "setup" },
            { "label": "Per request", "members": ["route", "handle"], "phase": "call" } ]
```

Setup steps are numbered S1, S2…, and the rest 1, 2…. Comparison rows take a `phase` the
same way.

### Views

A `tour` or `callstack` step can say **what** its focus page shows, never **where**. Chorong
lays the page out from a named template.

```jsonc
"views": {
  "handle": {
    "arrangement": "main-side",
    "panes": [
      { "kind": "editor", "highlights": [
          { "anchor": "handle", "name": "Signature first",
            "note": "The request is checked before anything is read from it." },
          { "anchor": "parse", "name": "Parsing after" } ] },
      { "kind": "graph", "of": "handle", "direction": "out", "depth": 1 }
    ]
  }
}
```

- Keys are step ids. A step without a view gets the **default page**: an editor at the
  step's anchor, plus Context. An external step has no code, so its default page has no
  panes.
- Pane kinds:
  - `editor`: `anchor`, or a list of `highlights`. Each has an `anchor`, an optional `name`
    (a two-to-five-word label, on the strip above the chunk) and an optional `note` (a
    sentence or two of commentary, under the label in the step's list of chunks). A chunk that
    needs more is a step of its own. Without `name`, a highlight's `note` is its label, as in
    files written before `name` existed. All highlights are in one file. No anchor and no
    highlights means the step's own anchor.
  - `context`: an `anchor`, shown pinned.
  - `outline`: a `path`.
  - `graph`: `of` a node (code or external) or an anchor, with `direction` `in`, `out` or
    `both` (the default) and `depth` 1 to 5 (default 1).
  - `callstack`: this investigation.
  - `diff`: one file between two revisions, `{ "kind": "diff", "path": "src/db/schema.rs",
    "base": "main", "head": "feature/orders" }`. `base` and `head` name a branch, tag or
    commit; without `head` it is the working tree. A name that starts with `-` or holds
    whitespace, `..`, `:` or `@{` is rejected when the file is read. Check resolves both with
    git and reports a revision it can't resolve, or a path on neither side; it counts as
    broken. The viewer shows it with the comparison view's change marks, and shows a card
    saying what could not be resolved instead of guessing another revision.
  - `file-tree`: an optional `path`.
- Anchors in panes are a node id or an anchor written in place, and are checked like node
  anchors. A broken one stays as a placeholder pane with the reason.
- Arrangements: `single`, `side-by-side`, `stacked`, `main-side`, `grid`. Without one, Chorong
  uses `main-side` for two or more panes, else `single`. There are no sizes, coordinates or
  ordering beyond the pane list.
- An unknown pane kind or arrangement still loads. `cho investigation check` reports it,
  and the step shows the default page.
- There is no separate kind for "read these files together": use a tour (one step is fine)
  whose view has several editor panes with highlights.

### Comparisons

`"kind": "comparison"` compares two or more paths. **Sides** are paths of node ids. **Rows**
align their steps by the job they do.

```jsonc
"sides": [
  { "id": "rest", "label": "REST handler", "path": ["rest-route", "rest-handle", "wrap"],
    "variants": [ { "id": "bulk", "label": "Bulk endpoint",
                    "rows": { "entry": { "step": "bulk-handle" } } } ] },
  { "id": "soap", "label": "SOAP handler", "path": ["soap-servlet", "wrap"] }
],
"rows": [
  { "id": "entry", "name": "Receive the request", "phase": "call",
    "cells": { "rest": { "step": "rest-handle" }, "soap": { "step": "soap-servlet" } },
    "result": "differs", "callout": "Paths split here" },
  { "id": "route", "name": "Register the endpoint", "phase": "setup",
    "cells": { "rest": { "step": "rest-route" }, "soap": { "missing": "mapped in deployment config" } } }
]
```

- Every row has one cell per side. A cell is a `step` (with optional `values` and `note`) or
  a `missing` reason.
- `result` is `same`, `differs` or `missing`:
  - **missing** is derived whenever a side has no step, and wins over a written result.
  - When every side anchors the same declaration, the row says so (`same_code`). It is
    **same** (derived) only when the author wrote no result and no values. Two sides often
    run the same code with different values, so a written `differs` stands (check notes
    "same code; differs by values").
  - Otherwise the result is the author's claim. Check reports a row with no result it can
    derive and none written.
- Check also reports a cell step that isn't on its side's path, and rows out of path order.
- A **variant** is another version of a side (one per tool, scenario or configuration). It
  overrides only the rows it names; an override may also set `result` and `callout`.

### Tour branches

A tour's top-level `variants` are branches. They use the same object, keyed by step id. A
`missing` step is shown as **not reached**; a `step` puts another node in its place.

```jsonc
"variants": [ { "id": "cached", "label": "Push already seen",
                "rows": { "enqueue": { "missing": "not reached: a cached push is not queued again" } } } ]
```

### Derived for every step

- **Depth:** call edges (`calls`, `dispatches`) from the first step. It is verified only
  along verified edges, and claimed otherwise.
- **Called by:** the `calls` and `dispatches` edges into the step, each with Chorong's label.

Authors don't write depth or "called by".

## Where investigations live

- **In the repository:** agents write files under `.chorong/investigations/` in the worktree
  being read. `cho investigation list` lists them; Chorong never writes to them.
- **From a link:** `chorong://investigation?src=SRC`, where `SRC` is an absolute path (no `.`
  or `..`) or an `https://` URL without credentials.
- **Any file**, named on the command line or opened in the viewer.

Investigation files are untrusted input. Chorong reads one only if it is a regular file of at
most 4 MB (inside the repository, nothing a symbolic link leads to outside it), and fetches
URLs over `https://` only, with a 4 MB limit, a 15-second timeout, no cookies or credentials, and redirects only
to other `https://` URLs. A fetched investigation is not stored; it is fetched again each
time it is opened.

## Checking against the pinned commit

`rev` says which commit the anchors were written against. When the checkout is at another
commit, `cho investigation check` says so and checks the working tree by default;
`--at rev` checks the pinned commit instead, reading each file from git (the commit must be
in the local repository). Labels always describe the code that was checked: Chorong never
re-anchors silently.

## Rendering

- `callstack` and `tour`: an ordered list beside the source; each step opens its anchor.
- `diagram` and `pr-overlay`: a graph laid out by Chorong (layered, top-down for calls). Files
  never carry coordinates, so the same file always gives the same picture.
- Edge styles: solid for verified, dashed for claimed, red for broken.

The viewer shows version 1 and version 2 files ([user guide](user-guide.md#tours-and-call-stacks)):

- Tours and call stacks show in their panes. A tour's focus pages come from its `views`.
- Several highlights in one editor are tinted chunks with numbered label strips, listed with
  their commentary under the step's note.
- External nodes show as cards, never with a verification label.
- Action steps show as cards with a Copy button per command; nothing is run.
- Diff panes show the file on its head side with the change marks of Compare against….
- Comparisons show in the Comparison tab, and a comparison can also open as a tour of paired
  steps.
- Variants and tour branches have pickers.
- Graph panes use an interim static drawing laid out by Chorong.

In a call stack, a frame after the first is verified only when the index confirms the call
into it from the frame before, which needs the working tree to be the code checked and a
current index; frames checked at the pinned commit are claimed at best. Showing diagrams and
PR overlays as their own kinds is planned.

## PR overlays

For a pull request Chorong builds its own **fact graph** deterministically: changed declarations
(from diff hunks), their callers and callees (from the index), and module grouping. A
`pr-overlay` investigation adds grouping, labels, reading order and notes on top. Overlay
nodes that aren't in the fact graph are drawn but marked as outside the computed change scope.

## Writing one (for skills and agents)

The full authoring guide ships as an agent skill: `cho skill investigation` prints it, and
`cho skill investigation --install .claude/skills` installs it. In short:

1. Pin `rev` to the exact commit you read.
2. Ask Chorong for the mechanical facts (`cho find`, `cho outline`, `cho references`,
   `cho graph`, `cho changes`) and write only the judgement on top.
3. Anchor declarations with `symbol`; add an `excerpt`, or a `from`/`to` range, for a
   specific spot. Never line numbers.
4. Use `calls` only for direct calls you saw in the source; otherwise `dispatches` with a note.
5. Write the file to `.chorong/investigations/` in the repository being read.
6. Run `cho investigation check FILE --root CHECKOUT --json` and fix every broken anchor.
