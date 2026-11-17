//! `cho formats`, `schema`, `skill` and `theme`: the file formats agents can write for Chorong.

use crate::{Output, skill};
use anyhow::{Context, Result, bail};
use cho_theme::ThemeFamily;
use serde_json::{Value, json};
use std::path::PathBuf;

pub fn theme(file: PathBuf) -> Result<Output> {
    let json = cho_core::fs::read_source_text(&file)
        .with_context(|| format!("cannot read {}", file.display()))?;
    let family = ThemeFamily::parse(&json)?;
    let themes: Vec<Value> = family
        .themes
        .iter()
        .map(|t| json!({ "name": t.name, "appearance": format!("{:?}", t.appearance).to_lowercase() }))
        .collect();
    let text = format!(
        "{} by {}\n{}",
        family.name,
        family.author,
        family
            .themes
            .iter()
            .map(|t| format!("  {} ({:?})", t.name, t.appearance))
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(Output::ok(
        text,
        json!({ "family": family.name, "author": family.author, "themes": themes }),
    ))
}

pub fn formats() -> Result<Output> {
    let formats = json!([{
        "name": "investigation",
        "version": cho_investigation::FORMAT_VERSION,
        "versions": (1..=cho_investigation::LATEST_FORMAT_VERSION).collect::<Vec<_>>(),
        "description": "Call stacks, code tours, diagrams and PR overlays anchored to declarations.",
        "file_extension": ".json",
        "schema_command": "cho schema investigation",
        "skill_command": "cho skill investigation",
        "check_command": "cho investigation check FILE --root REPO",
    }, {
        "name": "zed-theme",
        "version": "v0.2.0",
        "description": "Zed theme files; Chorong reads them unchanged.",
        "file_extension": ".json",
        "schema_url": "https://zed.dev/schema/themes/v0.2.0.json",
        "check_command": "cho theme FILE",
    }]);
    let text = "investigation  v1–2    call stacks, tours, diagrams, PR overlays, comparisons\n               schema: cho schema investigation   skill: cho skill investigation\nzed-theme      v0.2.0  Zed theme files, read unchanged (cho theme FILE)";
    Ok(Output::ok(text, json!({ "formats": formats })))
}

pub fn schema(format: String) -> Result<Output> {
    require_investigation(&format)?;
    let schema: Value = serde_json::from_str(INVESTIGATION_SCHEMA)?;
    Ok(Output::ok(INVESTIGATION_SCHEMA, schema))
}

pub fn skill_cmd(format: String, install: Option<PathBuf>, force: bool) -> Result<Output> {
    require_investigation(&format)?;
    let skill = investigation_skill();
    match install {
        None => Ok(Output::ok(
            skill.clone(),
            json!({ "name": "chorong-investigations", "version": skill::version(), "content": skill }),
        )),
        Some(dir) => {
            let path = dir.join("chorong-investigations").join("SKILL.md");
            if path.exists() && !force {
                bail!("{} exists; pass --force to overwrite", path.display());
            }
            std::fs::create_dir_all(path.parent().expect("has parent"))?;
            std::fs::write(&path, &skill)?;
            Ok(Output::ok(
                format!("wrote {} (skill {})", path.display(), skill::version()),
                json!({ "installed": path, "version": skill::version() }),
            ))
        }
    }
}

const INVESTIGATION_SCHEMA: &str = include_str!("../schema/investigation.schema.json");
fn investigation_skill() -> String {
    skill::text()
}

fn require_investigation(format: &str) -> Result<()> {
    match format {
        "investigation" => Ok(()),
        other => bail!("unknown format `{other}`; run `cho formats`"),
    }
}
