#!/usr/bin/env python3
"""Check relative Markdown links.

  scripts/check-doc-links.py            check the working tree (tracked *.md files)
  scripts/check-doc-links.py TREE       check a git tree-ish: every link in its Markdown files
                                        must resolve inside that tree. scripts/publish.sh uses
                                        this on the filtered tree, so a published page that
                                        links to an excluded path fails.

Also fails on an image under docs/public/ that no Markdown page uses.
Anchors are checked against the target's headings (GitHub slug rules). Exit 1 on any problem.
"""
import posixpath, re, subprocess, sys

def git(*a, text=True):
    return subprocess.check_output(["git", *a], text=text)

tree = sys.argv[1] if len(sys.argv) > 1 else None
if tree:
    paths = git("ls-tree", "-r", "--name-only", tree).splitlines()
    read = lambda p: git("show", f"{tree}:{p}")
else:
    paths = git("ls-files").splitlines()
    read = lambda p: open(p, encoding="utf-8").read()
exists = set(paths)
dirs = {posixpath.dirname(p) for p in paths} | {""}
for d in list(dirs):
    while d:
        d = posixpath.dirname(d); dirs.add(d)

LINK = re.compile(r"!?\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")

def strip_code(text):
    out, fence = [], False
    for line in text.split("\n"):
        if line.startswith("```"):
            fence = not fence; out.append(""); continue
        out.append("" if fence else re.sub(r"`[^`]*`", "", line))
    return out

def slugs(text):
    out = set()
    for line in strip_code_keep_headings(text):
        m = re.match(r"#{1,6}\s+(.*)", line)
        if m:
            t = re.sub(r"[`*_]", "", m.group(1)).strip().lower()
            out.add(re.sub(r"[^\w\- ]", "", t).replace(" ", "-"))
    return out

def strip_code_keep_headings(text):
    fence = False
    for line in text.split("\n"):
        if line.startswith("```"):
            fence = not fence; continue
        if not fence:
            yield line

bad = 0
linked = set()
for f in paths:
    if not f.endswith(".md"):
        continue
    for n, line in enumerate(strip_code(read(f)), 1):
        for m in LINK.finditer(line):
            tgt = m.group(1)
            if re.match(r"[a-z][a-z0-9+.-]*:", tgt):
                continue
            path, _, frag = tgt.partition("#")
            dest = f if not path else posixpath.normpath(posixpath.join(posixpath.dirname(f), path))
            linked.add(dest)
            if dest not in exists and dest.rstrip("/") not in dirs:
                print(f"{f}:{n}: link to missing path {tgt}"); bad += 1
            elif frag and dest.endswith(".md") and frag not in slugs(read(dest)):
                print(f"{f}:{n}: no heading #{frag} in {dest}"); bad += 1
# Public screenshots are intentional: an image under docs/public/ must be used by some Markdown page.
# Internal captures (layout lab, perf runs) go under design/img/.
for p in sorted(paths):
    if p.startswith("docs/public/") and re.search(r"\.(png|jpe?g|gif|mp4|webm|svg)$", p) and p not in linked:
        print(f"{p}: image under docs/public/ is not used by any Markdown page (public images must be "
              "intentional; internal captures go in design/img/ or docs/img/)"); bad += 1
sys.exit(1 if bad else 0)
