//! The tool settings for indexing and prepare: [`config`] reads them for a build, and
//! [`save_found`] writes back a `node`, `npm` or `uv` that Chorong found because its setting was
//! empty, so the search runs once.

use cho_semantic::tools::{Program, ToolConfig};
use std::ffi::OsString;
use std::path::Path;

/// The build's [`ToolConfig`] from `settings`, saving what it finds.
pub fn config(settings: &cho_settings::Settings) -> ToolConfig {
    ToolConfig {
        save: Some(save_found),
        ..ToolConfig::from_lookup(|k| settings.get(k).map(|v| v.to_string()))
    }
}

/// Saves `path` as the global value of `key` (`tools.node`, `tools.npm`, `tools.uv`) in
/// `settings.json`. Only fills an empty setting: one the reader set meanwhile is kept.
pub fn save_found(key: &'static str, path: &Path) {
    let mut store = cho_settings::SettingsStore::open_default();
    let empty = store
        .global_value(key)
        .is_none_or(|v| v.to_string().trim().is_empty());
    if empty {
        store
            .set(
                cho_settings::Scope::Global,
                key,
                cho_settings::Value::text(path.to_string_lossy()),
            )
            .ok();
    }
}

/// The programs a recipe's steps start that Chorong can find for it: `uv`, and `node` for the
/// Node package managers (whose `#!/usr/bin/env node` needs it on `PATH`).
fn programs_for(argv0: &[&str]) -> Vec<Program> {
    let mut out = vec![];
    for p in argv0 {
        let want = match *p {
            "uv" | "uvx" => Program::Uv,
            "node" | "npm" | "npx" | "pnpm" | "yarn" | "corepack" => Program::Node,
            _ => continue,
        };
        if !out.contains(&want) {
            out.push(want);
        }
    }
    out
}

/// `env` with the folders of the programs the recipe's steps start (`uv`, `node`) first on
/// `PATH`, from their settings or found once (and saved), never from the repository at
/// `worktree`. A program that is not found is left to the step's own check, which reports it
/// missing; the search's report is returned for its log.
pub fn with_tool_dirs(
    mut env: Vec<(OsString, OsString)>,
    argv0: &[&str],
    cfg: &ToolConfig,
    worktree: &Path,
) -> (Vec<(OsString, OsString)>, Vec<String>) {
    let machine = cho_semantic::tools::Machine::current();
    let mut dirs: Vec<std::path::PathBuf> = vec![];
    let mut notes = vec![];
    for program in programs_for(argv0) {
        match cho_semantic::tools::resolve(program, cfg, None, &[worktree], &machine) {
            Ok(p) => dirs.extend(p.parent().map(Path::to_path_buf)),
            Err(e) => notes.push(e),
        }
    }
    if dirs.is_empty() {
        return (env, notes);
    }
    let rest = env
        .iter()
        .rev()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let path =
        std::env::join_paths(dirs.into_iter().chain(std::env::split_paths(&rest))).unwrap_or(rest);
    env.retain(|(k, _)| k != "PATH");
    env.push(("PATH".into(), path));
    (env, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_programs() {
        assert_eq!(programs_for(&["uv"]), [Program::Uv]);
        assert_eq!(
            programs_for(&["pnpm", "uv", "npm"]),
            [Program::Node, Program::Uv]
        );
        assert!(programs_for(&["cargo", "make"]).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn tool_folders_go_first_on_path() {
        let d = std::env::temp_dir().join(format!("cho-tooldirs-{}", std::process::id()));
        std::fs::create_dir_all(d.join("bin")).unwrap();
        std::fs::write(d.join("bin/uv"), "").unwrap();
        let cfg = ToolConfig {
            uv: Some(d.join("bin/uv")),
            ..ToolConfig::default()
        };
        let env = vec![("PATH".into(), "/usr/bin".into())];
        let (env, notes) = with_tool_dirs(env, &["uv"], &cfg, Path::new("/repo"));
        assert!(notes.is_empty());
        let path = env.iter().find(|(k, _)| k == "PATH").unwrap().1.clone();
        assert_eq!(
            path,
            OsString::from(format!("{}:/usr/bin", d.join("bin").display()))
        );
        // A recipe without uv or node leaves PATH alone.
        let (env2, _) = with_tool_dirs(env.clone(), &["cargo"], &cfg, Path::new("/repo"));
        assert_eq!(env2, env);
        std::fs::remove_dir_all(d).ok();
    }
}
