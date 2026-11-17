//! `cho settings`: show and change Chorong's own settings. Writes only Chorong's config
//! directory, never a repository.

use crate::{Output, SettingsCmd};
use anyhow::{Result, anyhow};
use cho_settings::{
    Diagnostic, DiagnosticKind, FileKind, FileStatus, Scope, SettingDef, SettingsStore, Source,
    Value, def,
};
use serde_json::{Value as Json, json};
use std::path::PathBuf;

fn value_json(v: &Value) -> Json {
    match v {
        Value::Bool(b) => json!(b),
        Value::UInt(n) => json!(n),
        Value::Text(t) => json!(t),
    }
}

fn kind_json(d: &SettingDef) -> Json {
    match d.kind {
        cho_settings::Kind::Bool => json!({ "type": "bool" }),
        cho_settings::Kind::UInt { min, max } => json!({ "type": "uint", "min": min, "max": max }),
        cho_settings::Kind::Font { mono } => json!({ "type": "font", "mono": mono }),
        cho_settings::Kind::Path => json!({ "type": "path" }),
        cho_settings::Kind::Version => json!({ "type": "version" }),
        cho_settings::Kind::Choice { options } => json!({ "type": "choice", "options": options }),
    }
}

/// One setting as data: the effective value and everything that explains it.
fn entry_json(store: &SettingsStore, d: &SettingDef) -> Json {
    let e = store
        .effective()
        .entry(d.key)
        .expect("every definition resolves");
    json!({
        "key": d.key,
        "group": d.group(),
        "value": value_json(&e.value),
        "source": e.source.as_str(),
        "default": value_json(&d.default),
        "global": store.global_value(d.key).map(|v| value_json(&v)),
        "repo": store.repo_value(d.key).map(|v| value_json(&v)),
        "lightweight_overrode": e.underlying.as_ref().map(|(v, s)| json!({
            "would_be": value_json(v),
            "from": s.as_str(),
        })),
        "explanation": e.explanation(),
        "kind": kind_json(d),
        "description": d.description,
        "cost_hint": d.cost_hint,
        "cost_class": d.cost_class.as_str(),
        "lightweight_off": d.lightweight_off,
    })
}

fn diagnostic_json(d: &Diagnostic) -> Json {
    json!({
        "file": d.file.file_name(),
        "kind": match d.kind {
            DiagnosticKind::Malformed => "malformed",
            DiagnosticKind::Unreadable => "unreadable",
            DiagnosticKind::UnknownKey => "unknown-key",
            DiagnosticKind::InvalidValue => "invalid-value",
        },
        "key": d.key,
        "repo": d.repo,
        "message": d.message,
    })
}

fn file_json(store: &SettingsStore, which: FileKind) -> Json {
    let status = store.file_status(which);
    json!({
        "path": store.file_path(which),
        "status": status.as_str(),
        "error": match status {
            FileStatus::Malformed(m) | FileStatus::Unreadable(m) => Some(m.clone()),
            _ => None,
        },
    })
}

fn open(repo: &Option<PathBuf>) -> Result<SettingsStore> {
    let mut store = SettingsStore::open_default();
    if let Some(dir) = repo {
        store
            .set_active_repo(Some(dir))
            .map_err(|e| anyhow!("{e}"))?;
    }
    Ok(store)
}

fn lookup(key: &str) -> Result<&'static SettingDef> {
    if let Some(why) = cho_settings::retired(key) {
        return Err(anyhow!("`{key}` was retired: {why}"));
    }
    def(key).ok_or_else(|| {
        let near: Vec<&str> = cho_settings::keys()
            .filter(|k| {
                k.contains(key) || key.contains(k) || k.split('.').next() == key.split('.').next()
            })
            .take(5)
            .collect();
        if near.is_empty() {
            anyhow!("unknown setting `{key}` (run `cho settings` to list them)")
        } else {
            anyhow!("unknown setting `{key}`; did you mean {}?", near.join(", "))
        }
    })
}

pub fn run(action: Option<SettingsCmd>, repo: Option<PathBuf>) -> Result<Output> {
    match action {
        None => list(&repo),
        Some(SettingsCmd::Get { key }) => get(&repo, &key),
        Some(SettingsCmd::Set { key, value }) => {
            let d = lookup(&key)?;
            let v = d.kind.parse(&value).map_err(|e| anyhow!("{key}: {e}"))?;
            write(&repo, d, |store, scope| store.set(scope, d.key, v))
        }
        Some(SettingsCmd::Unset { key }) => {
            let d = lookup(&key)?;
            write(&repo, d, |store, scope| store.unset(scope, d.key))
        }
    }
}

fn list(repo: &Option<PathBuf>) -> Result<Output> {
    let store = open(repo)?;
    let diags = store.diagnostics();
    let broken = diags.iter().any(|d| {
        matches!(
            d.kind,
            DiagnosticKind::Malformed | DiagnosticKind::Unreadable
        )
    });

    let mut text = String::new();
    let width = cho_settings::keys().map(str::len).max().unwrap_or(0);
    for (d, e) in store.effective().iter() {
        let note = match &e.underlying {
            Some((v, s)) => format!(" (would be {v}, from {})", s.as_str()),
            None => String::new(),
        };
        text.push_str(&format!(
            "{:<width$}  {:<6}  {}{note}\n",
            d.key,
            e.value.to_string(),
            e.source.as_str()
        ));
    }
    if let Some(r) = store.active_repo() {
        text.push_str(&format!("\nrepository: {r}\n"));
    }
    for d in &diags {
        text.push_str(&format!("warning: {d}\n"));
    }

    let unknown: Vec<Json> = store.unknown_keys().iter().map(diagnostic_json).collect();
    let json = json!({
        "config_dir": store.config_dir(),
        "files": {
            "settings": file_json(&store, FileKind::Settings),
            "repos": file_json(&store, FileKind::Repos),
        },
        "repo": store.active_repo(),
        "lightweight": store.effective().lightweight(),
        "version": store.version(),
        "settings": cho_settings::defs().iter().map(|d| entry_json(&store, d)).collect::<Vec<_>>(),
        "unknown_keys": unknown,
        "diagnostics": diags.iter().map(diagnostic_json).collect::<Vec<_>>(),
    });
    Ok(Output {
        text,
        json,
        ok: !broken,
    })
}

fn get(repo: &Option<PathBuf>, key: &str) -> Result<Output> {
    let d = lookup(key)?;
    let store = open(repo)?;
    let e = store.effective().entry(d.key).expect("resolves");
    Ok(Output::ok(e.value.to_string(), entry_json(&store, d)))
}

fn write(
    repo: &Option<PathBuf>,
    d: &SettingDef,
    f: impl FnOnce(
        &mut SettingsStore,
        Scope<'_>,
    ) -> Result<cho_settings::ChangeSet, cho_settings::SaveError>,
) -> Result<Output> {
    let mut store = open(repo)?;
    let scope = match repo {
        Some(dir) => Scope::Repo(dir),
        None => Scope::Global,
    };
    let change = f(&mut store, scope).map_err(|e| anyhow!("{e}"))?;
    let file = store
        .file_path(match repo {
            Some(_) => FileKind::Repos,
            None => FileKind::Settings,
        })
        .map(|p| p.to_path_buf());
    let e = store.effective().entry(d.key).expect("resolves");
    let mut json = entry_json(&store, d);
    json["scope"] = json!(if repo.is_some() { "repo" } else { "global" });
    json["file"] = json!(file);
    json["changed"] = json!(!change.is_empty());
    let mut text = format!("{} = {} ({})", d.key, e.value, e.source.as_str());
    if e.source == Source::Lightweight {
        text.push_str("; saved, but Lightweight mode is on, so it is off for now");
    } else if e.source == Source::Global && repo.is_some() {
        text.push_str("; this repository's override was removed");
    }
    Ok(Output::ok(text, json))
}
