//! `cho editor list` and `editor open`: other editors, and opening a location in one.

use crate::Output;
use crate::open_cmd::launch_or_print;
use anyhow::{Result, anyhow};
use cho_core::{Editor, EditorLocation};
use serde_json::{Value, json};
use std::path::PathBuf;

pub fn list() -> Result<Output> {
    let editors: Vec<Value> = Editor::builtins()
        .iter()
        .map(|e| json!({ "name": e.name(), "program": e.program(), "installed": on_path(e.program()) }))
        .collect();
    let text = editors
        .iter()
        .map(|e| {
            format!(
                "{:<20} {:<10} {}",
                e["name"].as_str().unwrap(),
                e["program"].as_str().unwrap(),
                if e["installed"] == true {
                    "installed"
                } else {
                    "-"
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Output::ok(text, json!({ "editors": editors })))
}

pub fn open(
    file: PathBuf,
    line: Option<usize>,
    column: Option<usize>,
    editor: String,
    project: Option<PathBuf>,
    dry_run: bool,
) -> Result<Output> {
    let editor = Editor::builtins()
        .into_iter()
        .find(|e| e.program() == editor)
        .ok_or_else(|| anyhow!("unknown editor `{editor}`; see `cho editor list`"))?;
    let path = std::path::absolute(&file)?;
    let mut loc = EditorLocation::new(path);
    if let Some(line) = line {
        loc = loc.at(line, column.unwrap_or(1));
    }
    let launch = match &project {
        Some(root) => editor.open_project(&std::path::absolute(root)?, Some(&loc)),
        None => editor.open_file(&loc),
    };
    launch_or_print(launch, dry_run)
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}
