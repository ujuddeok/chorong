#!/usr/bin/env python3
"""Assemble changelog.d/<section>/*.md fragments into CHANGELOG.md.

  scripts/changelog.py                    print the Unreleased section as it would be released
  scripts/changelog.py --release VERSION  merge fragments into Unreleased, rename it to
                                          "VERSION (YYYY-MM-DD)", add a fresh Unreleased, delete
                                          the fragments
"""
import argparse
import datetime
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HEADINGS = {"settings": "Settings", "viewer": "Viewer", "graph": "Graph",
            "semantics": "Semantics", "cli": "CLI (`cho`)", "investigations": "Investigations"}
ORDER = list(HEADINGS)


def fragments(frag_dir):
    """{heading: [bullet text, ...]} in a stable order (section order, then file name)."""
    out = {}
    if not frag_dir.is_dir():
        return out
    sections = sorted((d for d in frag_dir.iterdir() if d.is_dir()),
                      key=lambda d: (ORDER.index(d.name) if d.name in ORDER else len(ORDER), d.name))
    for d in sections:
        items = [f.read_text().strip() for f in sorted(d.glob("*.md")) if f.read_text().strip()]
        if items:
            out[HEADINGS.get(d.name, d.name.replace("-", " ").capitalize())] = items
    return out


def merge(changelog, frags):
    """Insert fragment bullets at the top of their ### section inside ## Unreleased
    (creating the section if needed). Returns the new text."""
    m = re.search(r"^## Unreleased\n", changelog, re.M)
    if not m:
        raise SystemExit("CHANGELOG.md has no '## Unreleased' section")
    start = m.end()
    nxt = re.search(r"^## ", changelog[start:], re.M)
    end = start + nxt.start() if nxt else len(changelog)
    body = changelog[start:end]
    for heading, items in frags.items():
        block = "\n".join(items) + "\n"
        h = re.search(rf"^### {re.escape(heading)}\n", body, re.M)
        if h:
            body = body[:h.end()] + block + body[h.end():]
        else:
            body = body.rstrip("\n") + f"\n\n### {heading}\n{block}\n"
    if not body.endswith("\n\n") and nxt:
        body = body.rstrip("\n") + "\n\n"
    return changelog[:start] + body + changelog[end:]


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--release", metavar="VERSION")
    ap.add_argument("--root", type=Path, default=ROOT, help=argparse.SUPPRESS)
    a = ap.parse_args(argv)
    path, frag_dir = a.root / "CHANGELOG.md", a.root / "changelog.d"
    frags = fragments(frag_dir)
    text = merge(path.read_text(), frags)
    if not a.release:
        m = re.search(r"^## Unreleased\n(.*?)(?=^## |\Z)", text, re.M | re.S)
        sys.stdout.write("## Unreleased\n" + m.group(1))
        return 0
    date = datetime.date.today().isoformat()
    text = text.replace("## Unreleased\n", f"## Unreleased\n\n## {a.release} ({date})\n", 1)
    path.write_text(text)
    for d in frag_dir.iterdir():
        if d.is_dir():
            for f in d.glob("*.md"):
                f.unlink()
            if not any(d.iterdir()):
                d.rmdir()
    print(f"CHANGELOG.md: released {a.release} with {sum(map(len, frags.values()))} fragment(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
