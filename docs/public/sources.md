# Remote sources

Chorong opens branches, pull requests and commits of GitHub and Bitbucket repositories without a
clone of your own. It keeps its own copy in a cache, reads a pull request as **Before** and
**After**, and never writes to your repositories. The viewer opens them with **Open Remote…**
([user guide](user-guide.md#opening-a-remote-repository)); this page covers the CLI
(`cho open`, `cho project`, `cho pr`, `cho auth`, `cho cache`) and what it does on disk.

Hosts: GitHub (`github.com`) and Bitbucket Cloud (`bitbucket.org`). Bitbucket Data Center is
supported by the same code but not tested. Pull requests from forks: on GitHub they are fetched
through `refs/pull/N/head` and may work (not tested); on Bitbucket Cloud Chorong says they aren't
supported yet.

## Opening

```
cho open github:acme/api --branch feat/login
cho open github:acme/api --pr 123
cho open github:acme/api --rev 3f9c1e2
cho open https://github.com/acme/api/pull/123
cho open https://bitbucket.org/acme/api/src/main/src/lib.rs#lines-10
cho open github:acme/api --branch feat/login --base release
```

Each command fetches what it needs, places a worktree at the head and prints its path, then
After (the head), Before and how the head compares with the target branch:

```
~/.cache/chorong/repos/github/acme/api/worktrees/pr-123
github:acme/api · pull request 123 "Add login" (open) by someone, feat/login → main
After   a71d004  Add login form
Before  3f9c1e2  Release 2.3  (merge base with main)
main at 9b0c2d1, fetched just now; 3 behind, 2 ahead
3 commits behind main; must be rebased before merging
```

`--view` then opens the worktree in the viewer, as `cho PATH` does; without it Chorong only prints
(for scripts and agents).

`--json` prints the same as data: `worktree`, `kind` (`branch`, `pull-request`, `commit`),
`head`, `before` and `target` (each `id`, `short`, `subject`, `author`, `time`), `base_source`
(`merge-base` or `override`), `merge_bases`, `target_branch`, `behind`, `ahead`,
`target_fetched_at`, `offline`, `pull_request`, `pull_request_fetched_at`,
`pull_request_stale`, `path`, `line`, `notes` and `evicted`.

What a target can be:

| Input | Opens |
| --- | --- |
| `--branch B` | that branch |
| `--pr N` | pull request N |
| `--rev SHA` | that commit (7 to 40 hexadecimal digits) |
| a repository, branch, file, commit or pull-request URL | what it names; a file URL also reports `path` and `line` |
| a clone URL (`git@github.com:acme/api.git`, `ssh://…`, `https://…/scm/P/r.git`) | the default branch |
| a `chorong://open` link with `rev` | that commit |

URLs are recognised for GitHub (`tree/`, `blob/`, `commit/`, `pull/`), Bitbucket Cloud (`src/`,
`branch/`, `commits/`, `pull-requests/`) and Bitbucket Data Center (`projects/P/repos/R`,
`browse?at=`, `commits/`, `pull-requests/`, `scm/P/R.git`, with a context path). For a URL
whose branch name contains `/` (`tree/feat/x/src`), the branch is the longest prefix that is a
branch of the repository. Text that could be a branch or a commit (a branch called `3f9c1e2`)
is an error naming both, never a guess.

Opening a repository that is not a project adds it as one.

## Before and After

- **After** is the head: the branch tip, the pull request's head or the commit.
- **Before** is the merge base of the head and the **target branch**: the pull request's
  destination, else the repository's default branch. It is computed locally from the fetched
  commits, so the same head and target always give the same Before, offline too. When there
  are several merge bases, Chorong takes the newest by committer time (ties go to the lowest id)
  and says so in a note.
- `--base REV` (a branch or commit) replaces Before; After stays the head.
- **behind** counts commits on the target branch that the head doesn't have; **ahead** the
  reverse. A pull request behind its target must be rebased before a fast-forward merge.
- The target branch is fetched on every open, and `target_fetched_at` records when. If the
  remote can't be reached, Chorong uses the last fetch and says so (`offline: true`).
- A pull request's details are kept with its worktree. When the host can't be reached, the
  saved copy is shown, marked stale (`pull_request_stale`, with when it was read), and its
  target branch is still used.
- When the pull request's details can't be read and none are saved (no network to the API,
  no token for a private repository), a GitHub pull request still opens from `refs/pull/N/head`, compared
  with the default branch, with a note. A Bitbucket Cloud pull request needs its details (its
  source branch) and fails with the reason.

`cho graph --pr REPO N` opens the pull request the same way and shows the blast radius of
Before..After from the worktree's semantic index. Index the worktree first (`cho open REPO
--pr N --prepare`, or `cho index --root WORKTREE` once the project is approved); Chorong does
not index it by itself, because indexing runs the project's build code.

## Prepare

Indexing a TypeScript monorepo needs `node_modules`, and often a build for generated code.
For remote projects, in Chorong's own worktrees only, Chorong can run the commands that do that
before indexing. It never does this in your own folders: there `cho index` names the
command for you to run (for example `pnpm install --frozen-lockfile`) when `node_modules` is
missing.

**A recipe** is an ordered list of steps. Each step is a command run as an argument list with
no shell (write `["sh", "-c", "…"]` for one), in a folder of the repository, with a timeout:

```json
{ "steps": [
  { "argv": ["pnpm", "install", "--frozen-lockfile"] },
  { "argv": ["pnpm", "-r", "build"], "cwd": ".", "timeout_s": 900 }
] }
```

`cwd` defaults to `.` and must stay inside the repository; `timeout_s` defaults to 1800.

**Where the recipe comes from**, highest first:

1. the project's own recipe (`cho prepare set`), kept in `projects.json`;
2. the repository's `.chorong/prepare.json`, read from the commit being prepared (not the
   checkout) and always shown before it runs;
3. a suggestion from a lockfile at the repository root: `pnpm-lock.yaml` →
   `pnpm install --frozen-lockfile`, `yarn.lock` → `yarn install --immutable`,
   `package-lock.json` → `npm ci` (with several, in that order, and a note); and `uv.lock` →
   `uv sync --frozen` (exactly the lockfile, into the project's `.venv`; it may download the
   Python that `.python-version` names), after the JavaScript install when there is one.
   Approving a suggestion keeps it as the project's own recipe;
4. none: indexing runs as it is.

**Approval.** One approval per project covers prepare **and** indexing, because indexers run
project code too (build scripts, procedural macros). It records a SHA-256 of the recipe and of
the **trigger files** at that commit: every `package.json`, the lockfiles (`pnpm-lock.yaml`,
`yarn.lock`, `package-lock.json`, `npm-shrinkwrap.json`, `Cargo.lock`, `uv.lock`), every
`build.rs`, `pyproject.toml` and `.python-version`, the `Cargo.toml` of each procedural-macro
crate, and `.chorong/prepare.json`. When the recipe or any
trigger file differs at the commit being opened (a pull request that changes a lockfile, say),
Chorong asks again and lists exactly which files were added, removed or changed. Without an
approval nothing from the project runs: no prepare and no index; the code is read with syntax
only. `cho index` on one of Chorong's worktrees is refused until the project is approved, and the
viewer asks for this approval instead of its own "Trust this folder?": a cache worktree is
trusted to index exactly when its project's approval covers it. Trusting one of your own
folders never approves a cache worktree, or the reverse.

**In the viewer.** Nothing prepares or indexes until you click. A window on one of Chorong's
worktrees with no index shows a banner with the recipe's commands and two buttons, **Prepare and
index** and **Index only**; Open Remote's **Prepare and index** checkbox (unchecked by default,
never remembered) is the same click for that open only. Either asks for this approval when it
doesn't cover the commit: a panel lists the recipe's commands and the new or changed trigger
files, and **Approve and prepare** stores the same approval as `cho prepare approve`. See
[Prepare and index](user-guide.md#prepare-and-index).

**Running.**

- Only in Chorong's own worktrees in the cache; any other folder is refused.
- `uv`, and `node` for `npm`, `pnpm` and `yarn`, come from the `tools.uv` and `tools.node`
  settings, found the first time and saved when empty, as for the indexers
  ([semantic.md](semantic.md#the-indexers-chorong-runs)); their folders go first on the steps'
  `PATH`, so a recipe works when Chorong was started from Finder or the Dock. A tool that is not
  found is reported missing with every place Chorong looked.
- One step after another; the first failure stops the run. Every step's program is checked
  first, so a missing tool runs nothing and says which (`pnpm is not installed`).
- A step that runs past its timeout is stopped with everything it started.
- The environment is the one indexers get ([semantic.md](semantic.md#indexing-from-the-viewer)):
  what a build needs (`PATH`, `HOME`, toolchain, compiler, proxy and locale settings) and none
  of the rest, with host tokens removed as well (`CHORONG_*TOKEN*`, `CHORONG_*EMAIL*`,
  `GITHUB_TOKEN`, `GH_TOKEN`, every `BITBUCKET_*`) and no git prompts. A private registry
  keeps working through its config file (`~/.npmrc`). `CARGO_TARGET_DIR` is one folder per
  project in the cache, shared by its worktrees; pnpm uses its normal shared store.
- Output goes to `<repos>/…/NAME/prepare/<worktree>.log`.
- Each worktree remembers its last run: the recipe and trigger-file hashes, the toolchain
  (`node`, `rustc`, and `pnpm`/`npm`/`yarn` when a step runs them, by `--version`), the result
  and the log. Prepare runs again only when one of those changes (or with `--force`).
- **A failed prepare never stops indexing.** The index is built anyway and its status reads
  `partial` (prepare failed, timed out, or a tool was missing), with the log path in the
  index's `meta.json` and in the output.

**Never automatically.** Prepare runs only when you ask: `cho prepare run`, `cho open
--prepare`, or a click in the viewer. The per-project auto-prepare and the global `prepare.auto`
setting were retired: an older `projects.json` with `auto_prepare` still loads (the field is
ignored and dropped on the next save), and `cho prepare auto` says it was removed.

```
cho prepare show github:acme/api [TARGET]      # recipe, its source, approval, trigger files, last run
cho prepare approve github:acme/api [TARGET]   # approve what `show` shows
cho prepare set github:acme/api -- pnpm install --frozen-lockfile
cho prepare set github:acme/api --file recipe.json
cho prepare run github:acme/api [TARGET] [--force]
cho prepare clear github:acme/api              # remove the recipe and approval
cho open github:acme/api --pr 123 --prepare    # open, prepare, then index
```

TARGET is a branch, `#PR`, commit or URL; the default branch when omitted. `--prepare` never
approves by itself: on a terminal it shows the recipe and asks, and otherwise it says what
needs approving and runs nothing.

## Projects

A project is a remote repository Chorong remembers, in `projects.json` in Chorong's config folder
(`~/.config/chorong`, macOS `~/Library/Application Support/chorong`; never in a repository), keyed
by its identity (`github:acme/api`).

| Field | Default | Meaning |
| --- | --- | --- |
| `protocol` | `https` | clone over `ssh` or `https` |
| `url` | none | clone from this URL instead (a mirror, a Data Center server under a context path) |
| `worktree_cap` | 10 | worktrees kept before the least recently opened is removed |
| `recipe` | none | the project's own [prepare](#prepare) recipe |
| `approval` | none | the approved recipe and trigger-file hashes |

`cho project add REPO|URL [--protocol ssh|https] [--url URL] [--worktree-cap N]`,
`cho project list`, `cho project remove REPO` (also removes its cache). The viewer's
**Settings, Projects** section edits the protocol and the worktree cap, and removes projects,
with the same effects.

## The cache

```
$XDG_CACHE_HOME/chorong/repos/            (macOS: ~/Library/Caches/chorong/repos/)
  github/acme/api.git                  bare clone without file contents (--filter=blob:none)
  github/acme/api/state.json           worktree recency, default branch, fetch times
  github/acme/api/worktrees/main       detached worktree for the default branch
  github/acme/api/worktrees/pr-123     one per pull request, branch or commit opened
```

`bitbucket/WORKSPACE/NAME` and `bitbucket-dc/HOST/PROJECT/NAME` follow the same shape.
Worktree folders are named `pr-N`, `rev-<12 hex>`, or the branch name (characters outside
`A-Z a-z 0-9 . _ -` become `-`, and a short hash is added when that changed the name).
`CHORONG_REPOS_DIR` replaces the `repos` folder (tests, CI).

- Each worktree `cho open` places is added to Chorong's recent folders, so `chorong://` links to
  the repository or the pull request open it.
- One clone per repository; worktrees share its objects. File contents are fetched when a
  worktree is placed or a file at Before is read.
- Opening one more worktree than the project's cap removes the least recently opened one and
  its index. The default branch's worktree is never removed this way.
- The clone stays until the project is removed or the cache is cleared.
- `cho cache list` shows disk use per repository and worktree; `cho cache clear` removes
  everything, `cho cache clear REPO` one repository, `cho cache clear REPO WORKTREE` one
  worktree (each with their indexes). **Settings, Cache** in the viewer shows the same sizes
  (measured when the section opens) and does the same cleanups, each after a confirmation.
- Chorong writes only inside this folder. Your own clones are never written to and never used as
  an object source.
- One open, fetch or clear at a time per repository: the viewer and the CLI take a lock
  (`.NAME.lock` beside the clone) and a second one waits for the first, up to ten minutes,
  then fails saying another Chorong is using the repository.
- Git LFS files appear as pointer files: Chorong doesn't download LFS content.

## git and gix

The `git` command (2.36 or newer; Chorong checks and says when it is older) does the network work
and the writes in the cache: `clone --bare --filter=blob:none`, `fetch`, `worktree add
--detach` and `worktree remove`. It runs with prompts, hooks and the file-system monitor
turned off, never recurses into submodules, and refuses a remote URL git could read as an
option or a helper command (`--upload-pack=…`, `ext::…`). gix reads commits, merge bases and
files. Opening fetches Before's copies of the changed files in one batch; any other file whose
contents were not fetched yet is fetched with `git cat-file` when read.

## Tokens

Public repositories need no token. For private ones, and to raise GitHub's quota for
unauthenticated requests (60 an hour):

| Host | Token | Environment fallback |
| --- | --- | --- |
| GitHub | fine-grained personal access token, read-only: Contents, Pull requests, Metadata | `CHORONG_GITHUB_TOKEN` |
| Bitbucket Cloud | Atlassian API token with read scopes for repositories and pull requests, with the account email | `CHORONG_BITBUCKET_TOKEN`, `CHORONG_BITBUCKET_EMAIL` |
| Bitbucket Data Center | HTTP access token with read access | `CHORONG_BITBUCKET_DC_TOKEN` |

```
cho auth add github.com                     # paste the token; it is not shown
cho auth add bitbucket.org --email EMAIL
cho auth status                             # source and validity, never the token
cho auth remove github.com
```

- The viewer can add, check and remove tokens too: **Settings, Accounts** (the token field is
  masked and the value goes to the keychain only).
- Tokens are stored in the OS keychain (macOS Keychain, Windows Credential Manager, the Secret
  Service on Linux), one entry per host. Where there is no keychain (a Linux machine without
  GNOME Keyring or KWallet, a container), `cho auth add` fails and says which environment
  variable to set instead. There is no plain-text store.
- A token never appears in settings, repositories, logs, links or on a command line.
- **git uses your own setup first** (SSH keys, credential helpers, proxies). Only if that fails
  for want of a credential, and Chorong has a token for the host, does it run the command once
  more with Chorong as the only credential helper (`cho credential`, which reads the token from
  the keychain or environment and answers git's protocol on standard input).
- Pull-request details are read over REST: number, title, author, state, source and target
  branch and head commit; nothing else. A token is sent only over HTTPS and never follows a
  redirect. `CHORONG_GITHUB_API_URL` and `CHORONG_BITBUCKET_API_URL`
  replace the API addresses (tests, proxies).
