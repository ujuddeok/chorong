//! `cho open`: hand files, folders and links to the `chorong-app` viewer, or fetch a remote
//! repository (`sources_cmd`); and launching programs for it and for `cho editor`.

use crate::{Output, sources_cmd};
use anyhow::{Context, Result, bail};
use cho_core::{ChorongLink, Launch};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;

/// `cho open`, local or remote.
pub fn run(
    targets: Vec<String>,
    name: Option<String>,
    compare: Option<String>,
    dry_run: bool,
    remote: sources_cmd::OpenRemote,
) -> Result<Output> {
    match compare {
        Some(_) if remote.picks_remote() => {
            bail!(
                "--compare opens a local folder; for a remote one use --base with --branch, --pr or --rev"
            )
        }
        None if sources_cmd::is_remote_open(&targets[0], &remote) => {
            if targets.len() > 1 {
                bail!("a remote repository opens on its own: give one target");
            }
            sources_cmd::open(&targets[0], &remote, dry_run)
        }
        compare => {
            let app = viewer_path();
            let mut args = open_args(targets, &mut std::io::stdin().lock())?;
            if let Some(name) = name {
                args.splice(0..0, ["--name".to_string(), name]);
            }
            if let Some(rev) = compare {
                // A ref is data: `--compare=REF` keeps one starting with `-` an argument.
                args.splice(0..0, [format!("--compare={rev}")]);
            }
            launch_or_print(
                Launch::Command {
                    program: app.display().to_string(),
                    args,
                },
                dry_run,
            )
        }
    }
}

/// The viewer's arguments for `cho open TARGETS`: a link alone, or absolute paths, with `-`
/// replaced by the paths read from `stdin` (one per line, blank lines skipped). Several targets
/// must all be files or folders, not links.
fn open_args(targets: Vec<String>, stdin: &mut impl std::io::BufRead) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for t in targets {
        if t == "-" {
            let mut text = String::new();
            stdin.read_to_string(&mut text)?;
            paths.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(String::from),
            );
        } else {
            paths.push(t);
        }
    }
    match paths.as_slice() {
        [] => bail!("no paths to open (standard input was empty)"),
        [one] if one.starts_with("chorong://") => {
            ChorongLink::parse(one)?;
            Ok(paths)
        }
        _ if paths.iter().any(|p| p.starts_with("chorong://")) => {
            bail!("a chorong:// link opens on its own: give it without other paths")
        }
        _ => paths
            .iter()
            .map(|p| Ok(std::path::absolute(p)?.display().to_string()))
            .collect(),
    }
}

/// The GUI viewer: `chorong-app` next to this executable, else on PATH.
pub fn viewer_path() -> PathBuf {
    let name = if cfg!(windows) {
        "chorong-app.exe"
    } else {
        "chorong-app"
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

pub fn launch_or_print(launch: Launch, dry_run: bool) -> Result<Output> {
    let Launch::Command { program, args } = &launch;
    let json = json!({ "program": program, "args": args, "launched": !dry_run });
    if !dry_run {
        Command::new(program)
            .args(args)
            .spawn()
            .with_context(|| format!("cannot run `{program}`"))?;
    }
    Ok(Output::ok(
        if dry_run {
            launch.to_string()
        } else {
            String::new()
        },
        json,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_reads_stdin_and_keeps_links_alone() {
        let mut input = std::io::Cursor::new("src/a.rs\n\n  src/b.rs  \n");
        let args = open_args(vec!["-".into(), "c.rs".into()], &mut input).unwrap();
        assert_eq!(args.len(), 3);
        assert!(args.iter().all(|a| std::path::Path::new(a).is_absolute()));
        assert!(args[0].ends_with("src/a.rs") && args[1].ends_with("src/b.rs"));
        assert!(args[2].ends_with("c.rs"));
        let none = &mut std::io::Cursor::new("");
        assert!(open_args(vec!["-".into()], none).is_err());
        let link = "chorong://open?repo=local:/x&path=a.rs".to_string();
        assert!(open_args(vec![link.clone(), "b.rs".into()], none).is_err());
        assert_eq!(open_args(vec![link.clone()], none).unwrap(), [link]);
    }
}
