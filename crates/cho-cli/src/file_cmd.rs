//! `cho outline`, `find`, `show` and `highlight`: one file, read and parsed (no index).

use crate::{Output, read_source};
use anyhow::{Context, Result, anyhow, bail};
use cho_core::SourceBuffer;
use cho_syntax::{Highlighter, Language, SymbolMatch, find_symbol, outline};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The language `outline` and `find` read declarations from. The highlight-only data formats
/// (JSON, TOML, XML) have none, so they are refused like any other unsupported file.
fn need_language(file: &Path, language: Option<Language>) -> Result<Language> {
    language.filter(|l| l.has_structure()).ok_or_else(|| {
        anyhow!(
            "{} is not a supported language (Rust, Swift, TypeScript, JavaScript, Python, Java)",
            file.display()
        )
    })
}

fn symbol_json(buffer: &SourceBuffer, language: Language, s: &cho_syntax::Symbol) -> Value {
    json!({
        "name": s.name,
        "qualified_name": s.qualified_name(language),
        "kind": s.kind.as_str(),
        "line": buffer.line_of(s.name_range.start) + 1,
        "end_line": buffer.line_of(s.range.end.saturating_sub(1)) + 1,
        "range": [s.range.start, s.range.end],
        "name_range": [s.name_range.start, s.name_range.end],
    })
}

pub fn outline_cmd(file: PathBuf) -> Result<Output> {
    let (buffer, language) = read_source(&file)?;
    let language = need_language(&file, language)?;
    let symbols = outline(language, buffer.text());
    let text = symbols
        .iter()
        .map(|s| {
            let indent = "  ".repeat(s.scope.len());
            format!(
                "{:>6}  {indent}{} {}",
                buffer.line_of(s.name_range.start) + 1,
                s.kind.as_str(),
                s.name
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let list: Vec<Value> = symbols
        .iter()
        .map(|s| symbol_json(&buffer, language, s))
        .collect();
    Ok(Output::ok(
        text,
        json!({ "file": file, "language": language.name(), "symbols": list }),
    ))
}

pub fn find(file: PathBuf, symbol: String) -> Result<Output> {
    let (buffer, language) = read_source(&file)?;
    let language = need_language(&file, language)?;
    let (status, hits) = match find_symbol(language, buffer.text(), &symbol) {
        SymbolMatch::Unique(s) => ("unique", vec![s]),
        SymbolMatch::Ambiguous(v) => ("ambiguous", v),
        SymbolMatch::NotFound => ("not-found", vec![]),
    };
    let text = if hits.is_empty() {
        format!("{symbol}: not found in {}", file.display())
    } else {
        hits.iter()
            .map(|s| {
                format!(
                    "{}:{}  {} {}",
                    file.display(),
                    buffer.line_of(s.name_range.start) + 1,
                    s.kind.as_str(),
                    s.qualified_name(language)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let matches: Vec<Value> = hits
        .iter()
        .map(|s| symbol_json(&buffer, language, s))
        .collect();
    Ok(Output {
        text,
        json: json!({ "status": status, "matches": matches }),
        ok: status != "not-found",
    })
}

pub fn show(file: PathBuf, lines: Option<String>) -> Result<Output> {
    let (buffer, _) = read_source(&file)?;
    let total = buffer.line_count();
    let (start, end) = parse_line_range(lines.as_deref(), total)?;
    let mut text = String::new();
    let mut list = Vec::new();
    for n in start..=end {
        let line = buffer.line_text(n - 1).unwrap_or_default();
        text.push_str(&format!("{n:>6}  {line}\n"));
        list.push(json!({ "line": n, "text": line }));
    }
    Ok(Output::ok(
        text,
        json!({ "file": file, "total_lines": total, "lines": list }),
    ))
}

pub fn highlight(file: PathBuf) -> Result<Output> {
    let (buffer, language) = read_source(&file)?;
    let language = language.ok_or_else(|| {
        anyhow!(
            "{} is not a highlighted language (Rust, Swift, TypeScript, JavaScript, \
             Python, Java, JSON, TOML, XML, Markdown)",
            file.display()
        )
    })?;
    let spans = Highlighter::new().highlight(language, buffer.text())?;
    let text = spans
        .iter()
        .map(|s| {
            let lc = buffer.byte_to_line_col(s.range.start);
            format!(
                "{}:{}  {:<22} {:?}",
                lc.line + 1,
                lc.col + 1,
                s.theme_key(),
                &buffer.text()[s.range.clone()]
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let list: Vec<Value> = spans
        .iter()
        .map(|s| json!({ "range": [s.range.start, s.range.end], "capture": s.capture_name(), "theme_key": s.theme_key() }))
        .collect();
    Ok(Output::ok(text, json!({ "file": file, "spans": list })))
}

fn parse_line_range(spec: Option<&str>, total: usize) -> Result<(usize, usize)> {
    let Some(spec) = spec else {
        return Ok((1, total));
    };
    let (a, b) = spec.split_once(':').unwrap_or((spec, spec));
    let parse = |s: &str, default: usize| -> Result<usize> {
        if s.is_empty() {
            Ok(default)
        } else {
            s.parse().with_context(|| format!("bad line number `{s}`"))
        }
    };
    let (start, end) = (parse(a, 1)?.max(1), parse(b, total)?.min(total));
    if start > end {
        bail!("empty line range {spec} (file has {total} lines)");
    }
    Ok((start, end))
}
