//! `cho definition`, `references`, `hover` and `symbols`: questions answered from the
//! semantic index.

use crate::{Output, read_source_under, require_index};
use anyhow::{Context, Result, anyhow};
use cho_core::SourceBuffer;
use cho_semantic::{IndexStatus, Location, Semantic};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn symbols(query: String, root: PathBuf, limit: usize) -> Result<Output> {
    let sem = require_index(&root)?;
    let hits = sem.search(&query, limit);
    let text = hits
        .iter()
        .map(|h| {
            format!(
                "{}:{}:{}  {} {}",
                h.location.path, h.location.line, h.location.column, h.kind, h.display_name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Output {
        text: if hits.is_empty() {
            format!("no symbol matches `{query}`")
        } else {
            text
        },
        json: json!({ "query": query, "index": sem.status(), "symbols": hits }),
        ok: !hits.is_empty(),
    })
}

fn location_json(root: &Path, l: &Location) -> Value {
    let snippet = cho_core::fs::read_source_in(root, Path::new(&l.path))
        .ok()
        .and_then(|b| SourceBuffer::from_bytes(b).ok())
        .and_then(|b| b.line_text(l.line - 1).map(|t| t.trim().to_string()));
    let mut v = json!({
        "path": l.path, "line": l.line, "column": l.column,
        "range": [l.range.start, l.range.end],
        "is_definition": l.is_definition, "text": snippet,
    });
    if let Some(m) = &l.mapped {
        v["mapped"] = json!({
            "symbol": m.symbol,
            "method": m.mapping.method,
            "through_reexports": m.mapping.through_reexports,
            "verified": false,
            "label": m.mapping.label(),
        });
    }
    v
}

/// A declaration's one-based line span and those lines' text, with the one-based
/// line span of the marked member when the declaration is an enclosing one.
struct Peek {
    start_line: usize,
    end_line: usize,
    /// Each line's text, prefixed `› ` when marked and `  ` otherwise if any is marked.
    lines: String,
    marked: Option<(usize, usize)>,
}

/// The lines of `d`'s declaration as the file is now, or `None` without a range.
fn read_peek(root: &Path, d: &cho_semantic::HoverDefinition) -> Option<Peek> {
    let found = d.declaration()?;
    let r = found.range();
    let buf = SourceBuffer::from_bytes(
        cho_core::fs::read_source_in(root, Path::new(&d.location.path)).ok()?,
    )
    .ok()?;
    let first = buf.line_of(r.start);
    let last = buf.line_of(r.end.saturating_sub(1).max(r.start));
    let marked = found.marked_range().map(|m| {
        (
            buf.line_of(m.start),
            buf.line_of(m.end.saturating_sub(1).max(m.start)),
        )
    });
    let lines = (first..=last)
        .filter_map(|n| {
            let text = buf.line_text(n)?;
            Some(match marked {
                Some((a, b)) if (a..=b).contains(&n) => format!("› {text}"),
                Some(_) => format!("  {text}"),
                None => text.to_string(),
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(Peek {
        start_line: first + 1,
        end_line: last + 1,
        lines,
        marked: marked.map(|(a, b)| (a + 1, b + 1)),
    })
}

/// A `FILE LINE:COL` position resolved against the root's index.
struct Position {
    canonical_root: PathBuf,
    /// Repository-relative, `/`-separated.
    rel: String,
    byte: usize,
    line: usize,
    col: usize,
    sem: Semantic,
}

/// Parses `LINE:COL` (one-based, column in UTF-8 bytes), finds FILE under the
/// root and loads the index.
fn resolve_position(file: &Path, position: &str, root: &Path) -> Result<Position> {
    let (line, col) = position
        .split_once(':')
        .and_then(|(l, c)| Some((l.parse::<usize>().ok()?, c.parse::<usize>().ok()?)))
        .filter(|(l, c)| *l >= 1 && *c >= 1)
        .ok_or_else(|| anyhow!("position must be LINE:COL, both one-based (got `{position}`)"))?;
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("cannot read {}", root.display()))?;
    // Relative paths are tried against the working directory, then the root.
    let candidate = if file.is_relative() && !file.exists() {
        canonical_root.join(file)
    } else {
        std::path::absolute(file)?
    };
    let abs = candidate
        .canonicalize()
        .with_context(|| format!("cannot read {}", file.display()))?;
    let rel_path = abs
        .strip_prefix(&canonical_root)
        .map_err(|_| anyhow!("{} is outside the root {}", file.display(), root.display()))?;
    let rel = rel_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let sem = require_index(&canonical_root)?;
    let buffer = read_source_under(&canonical_root, rel_path, &abs)?;
    let byte = buffer
        .line_col_to_byte(cho_core::LineCol {
            line: line - 1,
            col: col - 1,
        })
        .ok_or_else(|| anyhow!("{} has no line {line}", file.display()))?;
    Ok(Position {
        canonical_root,
        rel,
        byte,
        line,
        col,
        sem,
    })
}

/// Which of a symbol's locations `navigate` lists.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    /// `cho definition`.
    Definitions,
    /// `cho references`.
    References,
}

pub fn navigate(file: &Path, position: &str, root: &Path, nav: Nav) -> Result<Output> {
    let Position {
        canonical_root,
        rel,
        byte,
        line,
        col,
        sem,
    } = resolve_position(file, position, root)?;
    let stale = sem.status() != IndexStatus::Current;
    let fail = |reason: &str| Output {
        text: format!("{}:{line}:{col}: {reason}", file.display()),
        json: json!({ "found": false, "reason": reason, "index": sem.status() }),
        ok: false,
    };
    let Some(at) = sem.symbol_at(&rel, byte) else {
        return Ok(fail("no symbol at this position in the index"));
    };
    let locations = match nav {
        Nav::References => sem.references(&at.id),
        Nav::Definitions => sem.definitions(&at.id),
    };
    if locations.is_empty() {
        return Ok(fail(
            "the symbol has no definition in the index (external or generated code)",
        ));
    }
    let info = sem.info(&at.id);
    let name = info.map(|i| i.display_name.as_str()).unwrap_or("");
    // Each location's file is read once, for both the JSON and the text.
    let list: Vec<Value> = locations
        .iter()
        .map(|l| location_json(&canonical_root, l))
        .collect();
    let mut text = locations
        .iter()
        .zip(&list)
        .map(|(l, j)| {
            let snippet = j["text"].as_str().unwrap_or("");
            format!(
                "{}:{}:{}  {}{snippet}{}",
                l.path,
                l.line,
                l.column,
                if nav == Nav::References && l.is_definition {
                    "[def] "
                } else {
                    ""
                },
                l.mapped
                    .as_ref()
                    .map(|m| format!("  [{}]", m.mapping.label()))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if stale {
        text.push_str(&format!(
            "\n(index is {}; results may be out of date)",
            sem.status()
        ));
    }
    let key = match nav {
        Nav::References => "references",
        Nav::Definitions => "definitions",
    };
    let mut json = json!({
        "found": true,
        "symbol": at.id.symbol,
        "local": at.id.is_local(),
        "display_name": name,
        "kind": info.map(|i| i.kind.as_str()),
        "documentation": info.map(|i| i.documentation.clone()),
        "index": sem.status(),
    });
    json[key] = Value::Array(list);
    Ok(Output::ok(text, json))
}

/// `hover`: what the index records about the symbol at a position. Never a
/// synthesised type: no signature in the index means `signature: null` and a reason.
pub fn hover(file: &Path, position: &str, root: &Path, peek: bool) -> Result<Output> {
    let Position {
        canonical_root,
        rel,
        byte,
        line,
        col,
        sem,
    } = resolve_position(file, position, root)?;
    let found = if peek {
        sem.hover_at_with_peek(&rel, byte)
    } else {
        sem.hover_at(&rel, byte)
    };
    let Some(h) = found else {
        let reason = "no symbol at this position in the index";
        return Ok(Output {
            text: format!("{}:{line}:{col}: {reason}", file.display()),
            json: json!({ "found": false, "reason": reason, "index": sem.index_info(Some(&rel)) }),
            ok: false,
        });
    };
    let mut text = match &h.signature {
        Ok(sig) => sig.text.clone(),
        Err(why) => format!("(no signature in the index: {})", why.as_str()),
    };
    for doc in &h.documentation {
        text.push_str("\n\n");
        text.push_str(doc.trim_end());
    }
    let kind = if h.kind.is_empty() { "symbol" } else { &h.kind };
    text.push_str(&format!("\n\n{kind} {}", h.name));
    if !h.local_text.is_empty() && h.local_text != h.name {
        text.push_str(&format!(" (written `{}` here)", h.local_text));
    }
    let peeks: Vec<Option<Peek>> = h
        .definitions
        .iter()
        .map(|d| peek.then(|| read_peek(&canonical_root, d)).flatten())
        .collect();
    match h.definitions.as_slice() {
        [] => text.push_str("\ndefined outside the index"),
        defs => {
            for (d, p) in defs.iter().zip(&peeks) {
                let l = &d.location;
                text.push_str(&format!("\ndefined at {}:{}:{}", l.path, l.line, l.column));
                if let Some(m) = &l.mapped {
                    text.push_str(&format!(" [{}]", m.mapping.label()));
                }
                if !peek {
                    continue;
                }
                match (p, d.no_declaration()) {
                    (Some(p), _) => {
                        let label = match p.marked {
                            Some((a, b)) => format!(
                                "enclosing declaration, lines {}-{}, marked › {}",
                                p.start_line,
                                p.end_line,
                                if a == b {
                                    format!("line {a}")
                                } else {
                                    format!("lines {a}-{b}")
                                }
                            ),
                            None => format!("declaration, lines {}-{}", p.start_line, p.end_line),
                        };
                        text.push_str(&format!(" ({label})\n{}", p.lines));
                    }
                    (None, Some(why)) => {
                        text.push_str(&format!(" (no declaration: {})", why.as_str()))
                    }
                    (None, None) => {}
                }
            }
        }
    }
    let commit = h.index.commit.as_deref().map(|c| c.get(..12).unwrap_or(c));
    text.push_str(&format!(
        "\nindex: {}{}",
        h.index.status,
        commit.map(|c| format!(" at {c}")).unwrap_or_default()
    ));
    if h.index.status != IndexStatus::Current {
        text.push_str(" (results may be out of date)");
    }
    let definitions: Vec<Value> = h
        .definitions
        .iter()
        .zip(&peeks)
        .map(|(d, p)| {
            let mut v = location_json(&canonical_root, &d.location);
            if peek {
                v["declaration_range"] = match (d.declaration(), p) {
                    (Some(f), Some(p)) => json!({
                        "kind": f.kind().as_str(),
                        "start": f.range().start, "end": f.range().end,
                        "start_line": p.start_line, "end_line": p.end_line,
                        "marked_range": f.marked_range().zip(p.marked).map(|(m, (a, b))| json!({
                            "start": m.start, "end": m.end,
                            "start_line": a, "end_line": b,
                        })),
                    }),
                    _ => Value::Null,
                };
                v["no_declaration"] = json!(d.no_declaration());
            }
            v
        })
        .collect();
    let range = h.range.as_ref().map(|r| json!([r.start, r.end]));
    let json = json!({
        "found": true,
        "symbol": h.id.symbol,
        "local": h.id.is_local(),
        "name": h.name,
        "local_text": h.local_text,
        "display_name": h.name,
        "kind": h.kind,
        "range": range,
        "signature": h.signature.as_ref().ok(),
        "no_signature": h.signature.as_ref().err(),
        "documentation": h.documentation,
        "definitions": definitions,
        "index": h.index,
        "mapped": h.mapped.as_ref().map(|m| json!({
            "symbol": m.symbol,
            "method": m.mapping.method,
            "through_reexports": m.mapping.through_reexports,
            "verified": false,
            "label": m.mapping.label(),
        })),
    });
    Ok(Output::ok(text, json))
}
