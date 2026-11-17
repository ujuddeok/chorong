# Chorong

Chorong is a fast, read-only code viewer built for **reading** code, not editing it.

It opens repositories from GitHub, Bitbucket, or a local folder and helps you understand them:
syntax-aware structure, precise go-to-definition and references from prebuilt indexes,
pull-request review against the real merge base, and investigations (call stacks, code
tours, diagrams) that link straight into the source.

## Principles

- **Read-only.** No editing, no language servers. Structure comes from Tree-sitter; semantic
  navigation comes from batch [SCIP](https://github.com/sourcegraph/scip) indexes.
- **Deterministic.** What Chorong shows is computed from the code. Material written by an LLM
  (investigations, diagram overlays) is imported as data, checked against the index, and
  labelled as *verified*, *claimed*, or *broken*. Chorong never invents a link.
- **Exact positions.** UTF-8 byte offsets are the only position format; conversions happen at
  the edges.
- **Zed-flavoured.** Uses the Zed theme format, so any Zed theme works.
- **Edit elsewhere.** Any location opens in Zed, VS Code, Cursor, Xcode, JetBrains IDEs, or on
  GitHub/Bitbucket.
- **Agent-drivable.** A headless `cho` CLI with JSON output runs the same engine as the viewer.

## Languages

Rust, Swift, TypeScript/TSX/JavaScript, Python, and Java. JSON, TOML, XML and
Markdown are syntax-highlighted only.

## Quick start

On a Mac, [INSTALL.md](INSTALL.md) builds the app and the `cho` command and puts them on your
PATH. The `cho` command alone installs from crates.io with `cargo install chorong`. To try it
from the source folder:

```bash
cargo run -p chorong -- outline crates/cho-core/src/buffer.rs
cargo run -p chorong-app -- .          # the viewer (see docs/public/building.md for Linux packages)
```

## Status

Early scaffolding: local file viewing, highlighting, outline, Zed themes, external editors,
CLI, and the investigation format with anchor checking.

## Credits

Chorong is a reimagining of [Radiology](https://github.com/tqbf/radiology), a read-only macOS code
browser by Thomas Ptacek ([@tqbf](https://x.com/tqbf)), who encouraged others to build their own.
Many of Chorong's core ideas come from Radiology, including byte-exact navigation, SCIP-backed
semantics, Critical Lens, and code paths.

## Licence

Chorong is licensed under the [Functional Source License, Version 1.1, ALv2 Future
License](LICENSE.md) (FSL-1.1-ALv2). You may use, copy, change and share it for any purpose
except offering a competing commercial product or service. Each release also becomes available
under the Apache License 2.0 two years after it is published.

## Documentation

Start with [docs/public/README.md](docs/public/README.md).
