//! `cho search`: repository text search (ripgrep's libraries via `cho-search`).

use crate::Output;
use anyhow::{Result, anyhow};
use cho_search::{CancelToken, SearchOptions, search_all};
use serde_json::json;
use std::path::PathBuf;

pub struct SearchArgs {
    pub pattern: String,
    pub root: PathBuf,
    pub regex: bool,
    pub case_sensitive: bool,
    pub globs: Vec<String>,
    pub limit: usize,
}

pub fn run(a: SearchArgs) -> Result<Output> {
    if !a.root.is_dir() {
        return Err(anyhow!("{} is not a folder", a.root.display()));
    }
    let opts = SearchOptions {
        pattern: a.pattern.clone(),
        regex: a.regex,
        case_sensitive: a.case_sensitive,
        globs: a.globs,
        max_results: a.limit,
    };
    let (hits, summary) = search_all(&a.root, &opts, &CancelToken::new())?;
    let mut text = hits
        .iter()
        .map(|h| format!("{}:{}: {}", h.path, h.line, h.text.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    if hits.is_empty() {
        text = format!("no matches for `{}`", a.pattern);
    } else if summary.truncated {
        text.push_str(&format!("\n(stopped at {} results)", a.limit));
    }
    Ok(Output {
        text,
        json: json!({
            "pattern": a.pattern,
            "regex": a.regex,
            "case_sensitive": a.case_sensitive,
            "truncated": summary.truncated,
            "matches": hits,
        }),
        ok: !hits.is_empty(),
    })
}
