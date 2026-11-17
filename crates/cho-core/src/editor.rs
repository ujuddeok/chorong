//! Opening a location in another editor, or on the repository's web host.
//!
//! Chorong is read-only, so editing always happens elsewhere. This module only
//! *describes* how to open a location (a command line or a URL); the app
//! decides how to run it. See `docs/public/external-editors.md`.

use crate::link::RepoRef;
use std::fmt;
use std::path::{Path, PathBuf};

/// A location to open: an absolute file path plus an optional one-based
/// line and column (editors count from one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorLocation {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl EditorLocation {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            line: None,
            column: None,
        }
    }

    pub fn at(mut self, line: usize, column: usize) -> Self {
        self.line = Some(line.max(1));
        self.column = Some(column.max(1));
        self
    }

    /// `path`, `path:line` or `path:line:col`.
    fn path_line_col(&self) -> String {
        let path = self.path.display();
        match (self.line, self.column) {
            (Some(l), Some(c)) => format!("{path}:{l}:{c}"),
            (Some(l), None) => format!("{path}:{l}"),
            _ => path.to_string(),
        }
    }
}

/// An editor Chorong knows how to hand a location to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Editor {
    Zed,
    VsCode,
    Cursor,
    Sublime,
    Xcode,
    /// A JetBrains IDE, by its command-line launcher (`rustrover`, `webstorm`,
    /// `pycharm`, `idea`).
    JetBrains {
        launcher: String,
    },
    /// A user-defined command. Arguments may contain `{path}`, `{line}` and
    /// `{column}` placeholders; `{line}`/`{column}` default to `1`.
    Custom {
        name: String,
        program: String,
        args: Vec<String>,
    },
}

/// How to open something: the program to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    Command { program: String, args: Vec<String> },
}

/// `arg` as a shell would need it: as is when it holds only plain characters, else in single
/// quotes, so a printed command (`--dry-run`) can be pasted back into a shell.
fn shell_word(arg: &str) -> std::borrow::Cow<'_, str> {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_./:=@%+,-".contains(c);
    if !arg.is_empty() && arg.chars().all(plain) {
        arg.into()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''")).into()
    }
}

/// The command line, each word quoted only where a shell needs it.
impl fmt::Display for Launch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Launch::Command { program, args } = self;
        write!(f, "{}", shell_word(program))?;
        args.iter()
            .try_for_each(|a| write!(f, " {}", shell_word(a)))
    }
}

impl Editor {
    /// Every built-in editor, for settings menus.
    pub fn builtins() -> Vec<Editor> {
        let jb = |l: &str| Editor::JetBrains { launcher: l.into() };
        vec![
            Editor::Zed,
            Editor::VsCode,
            Editor::Cursor,
            Editor::Sublime,
            Editor::Xcode,
            jb("rustrover"),
            jb("webstorm"),
            jb("pycharm"),
            jb("idea"),
        ]
    }

    pub fn name(&self) -> String {
        match self {
            Editor::Zed => "Zed".into(),
            Editor::VsCode => "Visual Studio Code".into(),
            Editor::Cursor => "Cursor".into(),
            Editor::Sublime => "Sublime Text".into(),
            Editor::Xcode => "Xcode".into(),
            Editor::JetBrains { launcher } => match launcher.as_str() {
                "rustrover" => "RustRover".into(),
                "webstorm" => "WebStorm".into(),
                "pycharm" => "PyCharm".into(),
                "idea" => "IntelliJ IDEA".into(),
                other => other.into(),
            },
            Editor::Custom { name, .. } => name.clone(),
        }
    }

    /// The command-line program this editor installs, used to detect it.
    pub fn program(&self) -> &str {
        match self {
            Editor::Zed => "zed",
            Editor::VsCode => "code",
            Editor::Cursor => "cursor",
            Editor::Sublime => "subl",
            Editor::Xcode => "xed",
            Editor::JetBrains { launcher } => launcher,
            Editor::Custom { program, .. } => program,
        }
    }

    /// Opens a single file at a location using the editor's CLI.
    pub fn open_file(&self, loc: &EditorLocation) -> Launch {
        let path = loc.path.display().to_string();
        let line = loc.line.unwrap_or(1).to_string();
        let column = loc.column.unwrap_or(1).to_string();
        let args = match self {
            Editor::Zed | Editor::Sublime => vec![loc.path_line_col()],
            Editor::VsCode | Editor::Cursor => vec!["--goto".into(), loc.path_line_col()],
            Editor::Xcode => match loc.line {
                Some(_) => vec!["--line".into(), line, path],
                None => vec![path],
            },
            Editor::JetBrains { .. } => match loc.line {
                Some(_) => vec!["--line".into(), line, "--column".into(), column, path],
                None => vec![path],
            },
            Editor::Custom { args, .. } => args
                .iter()
                .map(|a| {
                    a.replace("{path}", &path)
                        .replace("{line}", &line)
                        .replace("{column}", &column)
                })
                .collect(),
        };
        Launch::Command {
            program: self.program().into(),
            args,
        }
    }

    /// Opens a whole project folder. `focus` optionally opens a file in it too.
    pub fn open_project(&self, root: &Path, focus: Option<&EditorLocation>) -> Launch {
        let root_arg = root.display().to_string();
        match (self, focus) {
            (Editor::Zed | Editor::Sublime, Some(loc)) => Launch::Command {
                program: self.program().into(),
                args: vec![root_arg, loc.path_line_col()],
            },
            (Editor::VsCode | Editor::Cursor, Some(loc)) => Launch::Command {
                program: self.program().into(),
                args: vec![root_arg, "--goto".into(), loc.path_line_col()],
            },
            (Editor::Custom { .. }, Some(loc)) => self.open_file(loc),
            (Editor::Custom { .. }, None) => self.open_file(&EditorLocation::new(root)),
            // Xcode and JetBrains open the project; the file follows if given.
            (_, Some(loc)) => self.open_file(loc),
            (_, None) => Launch::Command {
                program: self.program().into(),
                args: vec![root_arg],
            },
        }
    }
}

/// Percent-encodes characters that would break a URL path, keeping `/` and `:`.
fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A link to view a file on the repository's web host, at a pinned revision.
/// Returns `None` for local repositories.
pub fn web_url(repo: &RepoRef, rev: &str, path: &str, line: Option<usize>) -> Option<String> {
    let path = encode_path(path.trim_start_matches('/'));
    Some(match repo {
        RepoRef::GitHub { owner, name } => {
            let anchor = line.map(|l| format!("#L{l}")).unwrap_or_default();
            format!("https://github.com/{owner}/{name}/blob/{rev}/{path}{anchor}")
        }
        RepoRef::Bitbucket { workspace, name } => {
            let anchor = line.map(|l| format!("#lines-{l}")).unwrap_or_default();
            format!("https://bitbucket.org/{workspace}/{name}/src/{rev}/{path}{anchor}")
        }
        RepoRef::BitbucketDc {
            host,
            project,
            name,
        } => {
            let anchor = line.map(|l| format!("#{l}")).unwrap_or_default();
            format!("https://{host}/projects/{project}/repos/{name}/browse/{path}?at={rev}{anchor}")
        }
        RepoRef::Local { .. } => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc() -> EditorLocation {
        EditorLocation::new("/src/my app/main.rs").at(12, 5)
    }

    fn cmd(l: Launch) -> Vec<String> {
        let Launch::Command { program, mut args } = l;
        args.insert(0, program);
        args
    }

    #[test]
    fn file_commands() {
        assert_eq!(
            cmd(Editor::Zed.open_file(&loc())),
            ["zed", "/src/my app/main.rs:12:5"]
        );
        assert_eq!(
            cmd(Editor::VsCode.open_file(&loc())),
            ["code", "--goto", "/src/my app/main.rs:12:5"]
        );
        assert_eq!(
            cmd(Editor::Xcode.open_file(&loc())),
            ["xed", "--line", "12", "/src/my app/main.rs"]
        );
        assert_eq!(
            cmd(Editor::JetBrains {
                launcher: "rustrover".into()
            }
            .open_file(&loc())),
            [
                "rustrover",
                "--line",
                "12",
                "--column",
                "5",
                "/src/my app/main.rs"
            ]
        );
        let nvim = Editor::Custom {
            name: "Neovim (kitty)".into(),
            program: "kitty".into(),
            args: vec!["nvim".into(), "+{line}".into(), "{path}".into()],
        };
        assert_eq!(
            cmd(nvim.open_file(&loc())),
            ["kitty", "nvim", "+12", "/src/my app/main.rs"]
        );
    }

    #[test]
    fn file_without_position() {
        let l = EditorLocation::new("/a.py");
        assert_eq!(cmd(Editor::Zed.open_file(&l)), ["zed", "/a.py"]);
        assert_eq!(cmd(Editor::Xcode.open_file(&l)), ["xed", "/a.py"]);
    }

    #[test]
    fn project_commands() {
        let root = Path::new("/src/my app");
        assert_eq!(
            cmd(Editor::Zed.open_project(root, None)),
            ["zed", "/src/my app"]
        );
        assert_eq!(
            cmd(Editor::Cursor.open_project(root, Some(&loc()))),
            [
                "cursor",
                "/src/my app",
                "--goto",
                "/src/my app/main.rs:12:5"
            ]
        );
    }

    #[test]
    fn web_urls() {
        let gh = RepoRef::GitHub {
            owner: "acme".into(),
            name: "api".into(),
        };
        assert_eq!(
            web_url(&gh, "3f9c1e2", "src/lib.rs", Some(10)).unwrap(),
            "https://github.com/acme/api/blob/3f9c1e2/src/lib.rs#L10"
        );
        let bb = RepoRef::Bitbucket {
            workspace: "team".into(),
            name: "svc".into(),
        };
        assert_eq!(
            web_url(&bb, "abc", "a b.py", Some(3)).unwrap(),
            "https://bitbucket.org/team/svc/src/abc/a%20b.py#lines-3"
        );
        let dc = RepoRef::BitbucketDc {
            host: "git.example.com".into(),
            project: "PLAT".into(),
            name: "core".into(),
        };
        assert_eq!(
            web_url(&dc, "abc", "x.ts", None).unwrap(),
            "https://git.example.com/projects/PLAT/repos/core/browse/x.ts?at=abc"
        );
        assert_eq!(
            web_url(&RepoRef::Local { path: "/x".into() }, "abc", "x", None),
            None
        );
    }

    #[test]
    fn a_printed_command_quotes_only_what_a_shell_needs() {
        let l = Launch::Command {
            program: "/usr/bin/chorong-app".into(),
            args: vec![
                "--name".into(),
                "PR 412 review".into(),
                "/code/it's.rs".into(),
                "src/lib.rs:12".into(),
            ],
        };
        assert_eq!(
            l.to_string(),
            "/usr/bin/chorong-app --name 'PR 412 review' '/code/it'\\''s.rs' src/lib.rs:12"
        );
    }
}
