#!/usr/bin/env python3
"""Time `cho outline` on every source file of a repository and check budgets.

    python3 -I scripts/repo-budgets.py REPO [--cho target/release/cho] [options]

Standard library only. The repository is untrusted input: this script only walks it
and runs the cho binary on each file; it never executes anything from the repository.

Exit status: 0 within budget, 1 a budget was exceeded or a file crashed, 2 usage or
environment error (no cho binary, no source files). See docs/repo-budgets.md.
"""

import argparse
import concurrent.futures
import os
import subprocess
import sys
import time

# Extensions Chorong parses (crates/cho-syntax/src/language.rs).
EXTS = {
    ".rs": "Rust",
    ".swift": "Swift",
    ".ts": "TypeScript",
    ".mts": "TypeScript",
    ".cts": "TypeScript",
    ".tsx": "TSX",
    ".js": "JavaScript",
    ".jsx": "JavaScript",
    ".mjs": "JavaScript",
    ".cjs": "JavaScript",
    ".py": "Python",
    ".pyi": "Python",
    ".java": "Java",
}
REFUSALS = ("looks binary", "not valid UTF-8", "larger than")
SKIP_DIRS = {".git", "node_modules", "target", ".hg", ".svn"}


def collect(root, only):
    files = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        for name in sorted(filenames):
            lang = EXTS.get(os.path.splitext(name)[1])
            if lang is None or (only and lang not in only):
                continue
            path = os.path.join(dirpath, name)
            if os.path.islink(path):
                continue
            try:
                files.append((path, lang, os.path.getsize(path)))
            except OSError:
                pass
    return files


def run_one(cho, path, timeout):
    """Returns (seconds, problem or None). A problem is a crash, panic, timeout or non-zero exit."""
    start = time.perf_counter()
    try:
        proc = subprocess.run(
            [cho, "outline", path],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return time.perf_counter() - start, f"timeout after {timeout:g}s"
    secs = time.perf_counter() - start
    err = proc.stderr.decode("utf-8", "replace").strip().splitlines()
    tail = err[-1][:160] if err else ""
    if proc.returncode < 0:
        return secs, f"killed by signal {-proc.returncode} {tail}".strip()
    if b"panicked" in proc.stderr:
        return secs, f"panic: {tail}"
    if proc.returncode != 0:
        # Chorong refuses non-text input cleanly with exit 2 (UTF-16 or binary test fixtures are
        # common in real repositories). That is its contract, not a crash: report, don't fail.
        if proc.returncode == 2 and any(m in tail for m in REFUSALS):
            return secs, "refused:" + tail
        return secs, f"exit {proc.returncode}: {tail}"
    return secs, None


def pct(sorted_vals, p):
    if not sorted_vals:
        return 0.0
    i = min(len(sorted_vals) - 1, int(round(p / 100 * (len(sorted_vals) - 1))))
    return sorted_vals[i]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("repo", help="repository (or any directory) to sweep")
    ap.add_argument("--cho", default="target/release/cho", help="cho binary (default %(default)s)")
    ap.add_argument("--lang", action="append", default=[], metavar="NAME",
                    help="only this language (repeatable): " + ", ".join(sorted(set(EXTS.values()))))
    ap.add_argument("--jobs", type=int, default=1,
                    help="parallel runs (default 1; timings are only comparable at 1)")
    ap.add_argument("--top", type=int, default=5, help="slowest files to list (default %(default)s)")
    ap.add_argument("--max-file-secs", type=float, default=2.0,
                    help="budget: slowest single file (default %(default)s)")
    ap.add_argument("--max-ms-per-100kb", type=float, default=200.0,
                    help="budget: time per 100 KB, for files of at least --rate-min-bytes (default %(default)s)")
    ap.add_argument("--rate-min-bytes", type=int, default=50_000,
                    help="smaller files are exempt from the rate budget: process start-up dominates "
                         "them (default %(default)s)")
    ap.add_argument("--max-p99-ms", type=float, default=100.0,
                    help="budget: 99th percentile per file (default %(default)s)")
    ap.add_argument("--max-total-secs", type=float, default=0.0,
                    help="budget: whole sweep, 0 = no limit (default %(default)s)")
    ap.add_argument("--timeout", type=float, default=30.0,
                    help="kill one run after this long and count it a failure (default %(default)s)")
    args = ap.parse_args()

    cho = os.path.abspath(args.cho)
    if not os.access(cho, os.X_OK):
        print(f"repo-budgets: cho binary not found or not executable: {cho}", file=sys.stderr)
        return 2
    if not os.path.isdir(args.repo):
        print(f"repo-budgets: not a directory: {args.repo}", file=sys.stderr)
        return 2
    known = set(EXTS.values())
    bad = [n for n in args.lang if n not in known]
    if bad:
        print(f"repo-budgets: unknown language {bad}; choose from {sorted(known)}", file=sys.stderr)
        return 2

    files = collect(args.repo, set(args.lang))
    if not files:
        print("repo-budgets: no source files found", file=sys.stderr)
        return 2

    results = []  # (path, lang, size, secs, problem)
    t0 = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
        futs = {pool.submit(run_one, cho, p, args.timeout): (p, lang, size) for p, lang, size in files}
        for fut in concurrent.futures.as_completed(futs):
            p, lang, size = futs[fut]
            secs, problem = fut.result()
            results.append((p, lang, size, secs, problem))
    wall = time.perf_counter() - t0

    refused = [r for r in results if r[4] and r[4].startswith("refused:")]
    failures = []
    slow = []

    def rate(r):
        return r[3] * 1000 / (r[2] / 100_000) if r[2] else 0.0

    for r in results:
        if r[4] and not r[4].startswith("refused:"):
            failures.append(r)
        if r[3] > args.max_file_secs:
            slow.append((r, f">{args.max_file_secs:g}s"))
        elif r[2] >= args.rate_min_bytes and rate(r) > args.max_ms_per_100kb:
            slow.append((r, f"{rate(r):.0f} ms/100KB"))

    rel = lambda p: os.path.relpath(p, args.repo)
    print(f"{'language':<11}{'files':>7}{'MB':>8}{'total s':>9}{'p50 ms':>8}{'p99 ms':>8}{'max ms':>8}  slowest")
    p99_over = []
    for lang in sorted({r[1] for r in results}) + ["all"]:
        rs = [r for r in results if lang == "all" or r[1] == lang]
        ts = sorted(r[3] * 1000 for r in rs)
        top = sorted(rs, key=lambda r: -r[3])[: args.top if lang != "all" else 0]
        names = ", ".join(f"{rel(r[0])} ({r[2] / 1000:.0f}KB {r[3] * 1000:.0f}ms)" for r in top)
        total = sum(r[3] for r in rs)
        print(f"{lang:<11}{len(rs):>7}{sum(r[2] for r in rs) / 1e6:>8.1f}{total:>9.1f}"
              f"{pct(ts, 50):>8.0f}{pct(ts, 99):>8.0f}{ts[-1]:>8.0f}  {names}")
        if lang != "all" and pct(ts, 99) > args.max_p99_ms:
            p99_over.append((lang, pct(ts, 99)))

    over = False
    for r in sorted(failures, key=lambda r: r[0]):
        print(f"FAIL  {rel(r[0])} ({r[2]} B): {r[4]}")
    for r, why in sorted(slow, key=lambda x: -x[0][3]):
        print(f"SLOW  {rel(r[0])} ({r[2] / 1000:.0f} KB): {r[3] * 1000:.0f} ms, {why}")
    for lang, v in p99_over:
        print(f"SLOW  {lang} p99 {v:.0f} ms > {args.max_p99_ms:g} ms")
    if args.max_total_secs and wall > args.max_total_secs:
        print(f"SLOW  sweep took {wall:.1f}s > {args.max_total_secs:g}s")
        over = True
    over = over or bool(failures or slow or p99_over)
    if refused:
        print(f"note: {len(refused)} file(s) refused as binary or not UTF-8 (not failures), "
              f"e.g. {rel(refused[0][0])}")
    print(f"{'OVER BUDGET' if over else 'OK'}: {len(results)} files, wall {wall:.1f}s, jobs {args.jobs}")
    return 1 if over else 0


if __name__ == "__main__":
    sys.exit(main())
