//! `chorong-app`: the GPU-rendered viewer.
//!
//! The viewer links Chorong's crates directly and works on their in-memory types;
//! JSON is only for the CLI and agents. See `docs/public/architecture.md`.

mod actions;
mod assets;
mod blame;
mod callkind;
mod comparison;
mod context;
mod devlog;
mod document;
mod double_tap;
mod features;
mod files;
mod find;
mod fold;
mod generation;
mod graph;
mod history;
mod hover;
mod indexing;
#[cfg(target_os = "linux")]
mod instance;
mod investigation;
mod keymap;
mod lens;
mod links;
mod markers;
mod marks;
mod minimap;
mod note_md;
mod overscroll;
mod overview;
mod pane_style;
mod picker;
mod pointer;
mod prefs;
mod recent;
mod remote;
mod scroll;
mod search;
mod selection;
mod settings_window;
mod tour_pages;
mod tree;
mod trust;
mod view;

use anyhow::{Context, Result, anyhow, bail};
use cho_core::Editor;
use cho_theme::{Appearance, Theme, ThemeFamily};
use clap::Parser;
use futures::StreamExt;
use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowHandle,
    WindowOptions, px, size,
};
use links::{LinkInfo, Resolved};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "chorong-app", version, about = "Chorong viewer")]
struct Args {
    /// A file, a folder, or a `chorong://open` link to a local repository. Several files open in a
    /// focus layout: the first on the page, the rest in the tray.
    targets: Vec<String>,
    /// A name for the window's title ("PR 412 review — cho — Chorong"), so a window an agent
    /// opened can be told apart. A named window is always a new one.
    #[arg(long)]
    name: Option<String>,
    /// A Zed theme file (JSON, schema v0.2.0).
    #[arg(long)]
    theme: Option<PathBuf>,
    /// Which theme in the file to use. Defaults to the first.
    #[arg(long)]
    theme_name: Option<String>,
    /// Use the bundled light theme.
    #[arg(long)]
    light: bool,
    /// Editor for "Open in …": zed, code, cursor, subl, xed, rustrover, webstorm, pycharm, idea.
    #[arg(long, default_value = "zed")]
    editor: String,
    /// Monospace font family for source (default: the `appearance.code_font` setting).
    #[arg(long)]
    font: Option<String>,
    /// Start with every function body expanded.
    #[arg(long)]
    no_fold: bool,
    /// Start comparing with this branch, tag or commit (Before is the merge base of HEAD and
    /// it), as Compare Against… does. Always a new window.
    #[arg(long, value_name = "REF")]
    compare: Option<String>,
}

/// What to show first.
#[derive(Default)]
pub struct Startup {
    pub root: Option<PathBuf>,
    pub file: Option<PathBuf>,
    /// More files to open with `file`, given together: they go to the focus layout's tray.
    pub extra_files: Vec<PathBuf>,
    /// The window's name, shown first in its title (`--name`).
    pub name: Option<String>,
    /// Start comparing with this branch, tag or commit (`--compare`).
    pub compare: Option<String>,
    pub symbol: Option<String>,
    /// One-based line.
    pub line: Option<usize>,
    /// How the window was asked for.
    pub origin: Origin,
}

/// How a window was asked for: a plain path, or one of the ways a link or Open Remote leads.
#[derive(Default)]
pub enum Origin {
    /// A file or folder (or nothing).
    #[default]
    Plain,
    /// A `chorong://` link to a local worktree, for the commit-mismatch banner.
    Link(LinkInfo),
    /// A link to a repository with no worktree on this machine: its web page, if any.
    Elsewhere { info: LinkInfo, web: Option<String> },
    /// A pull-request link with no cache worktree for that pull request: the window asks
    /// before fetching it.
    AskPr(AskPr),
    /// A remote repository was opened into this worktree: start comparing with its merge base.
    Remote(Box<RemoteStart>),
    /// A `chorong://investigation` link: open it in the window for its repository.
    Investigation(investigation::Source),
}

impl Startup {
    /// The `chorong://` link this came from, if any.
    pub fn link(&self) -> Option<&LinkInfo> {
        match &self.origin {
            Origin::Link(info) => Some(info),
            Origin::Remote(r) => r.link.as_ref(),
            Origin::Plain
            | Origin::Elsewhere { .. }
            | Origin::AskPr(_)
            | Origin::Investigation(_) => None,
        }
    }
}

/// The question a pull-request link asks before Chorong fetches the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskPr {
    pub info: LinkInfo,
    pub repo: cho_core::RepoRef,
    pub number: u64,
    /// A local worktree at the link's commit, offered as a choice that fetches nothing.
    pub local: Option<PathBuf>,
}

/// The comparison an "Open Remote…" window starts with.
pub struct RemoteStart {
    /// Before, as a full commit id.
    pub before: String,
    /// What Before was compared against (the target branch or the reader's override).
    pub against: String,
    pub merge_bases: usize,
    /// What the remote header band shows (from the open's own result).
    pub header: remote::header::Header,
    /// The "Prepare and index" box was checked. Unchecked: this window is read-only, syntax
    /// only, and starts no prepare and no indexing.
    pub prepare: bool,
    /// What opening did besides open ("Added github:acme/api to your projects.").
    pub notes: Vec<String>,
    /// The `chorong://` link that asked for the open, if one did.
    pub link: Option<LinkInfo>,
}

impl Startup {
    /// What a `chorong://open` link leads to here (see `links::resolve`).
    pub fn from_link(url: &str) -> Result<Self> {
        let recent = cho_sources::recent::list();
        let mut s = Startup::default();
        match links::resolve(url, &recent).map_err(|e| anyhow!(e))? {
            Resolved::Local { root, info } => {
                s.file = info.path.as_ref().map(|f| root.join(f));
                s.root = Some(root);
                s.symbol = info.symbol.clone();
                s.line = info.line;
                s.origin = Origin::Link(info);
            }
            Resolved::Elsewhere { info, web } => s.origin = Origin::Elsewhere { info, web },
            Resolved::AskFetch {
                info,
                repo,
                number,
                local,
            } => {
                s.origin = Origin::AskPr(AskPr {
                    info,
                    repo,
                    number,
                    local,
                })
            }
            Resolved::Investigation(source) => {
                // A local file: its repository's worktree (the window it opens in). A URL is
                // read by the window it lands in, which shows its host if it cannot.
                if let investigation::Source::Path(path) = &source {
                    let inv = investigation::read(&source, None).map_err(|e| anyhow!(e))?;
                    s.root = investigation::worktree_for(&inv, Some(path), &recent);
                    if s.root.is_none() {
                        bail!(
                            "{} is not open in Chorong on this machine: open a clone of it once",
                            inv.repo
                        );
                    }
                }
                s.origin = Origin::Investigation(source);
            }
        }
        Ok(s)
    }
}

/// Several targets given together: files only (no folder or link among them), opened in one
/// window whose root is the repository holding them all (the nearest folder with a `.git`
/// above their shared folder), or that shared folder when there is no repository.
fn resolve_targets(targets: &[String]) -> Result<Startup> {
    let [first, rest @ ..] = targets else {
        return resolve_target(None);
    };
    if rest.is_empty() {
        return resolve_target(Some(first));
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for t in targets {
        if t.starts_with("chorong://") {
            bail!("a chorong:// link opens on its own, not with other paths");
        }
        let path = std::path::absolute(t)?;
        if path.is_dir() {
            bail!(
                "{} is a folder: open a folder on its own, or list files",
                path.display()
            );
        }
        if !path.is_file() {
            bail!("{} does not exist", path.display());
        }
        if !files.contains(&path) {
            files.push(path);
        }
    }
    let shared = files
        .iter()
        .filter_map(|f| f.parent())
        .map(Path::to_path_buf)
        .reduce(|a, b| {
            a.ancestors()
                .find(|anc| b.starts_with(anc))
                .map(Path::to_path_buf)
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let root = shared
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(&shared)
        .to_path_buf();
    let mut files = files.into_iter();
    Ok(Startup {
        root: Some(root),
        file: files.next(),
        extra_files: files.collect(),
        ..Startup::default()
    })
}

/// A hand-over line names one link or path; several paths are joined with tabs.
fn resolve_handover(line: &str) -> Result<Startup> {
    let parts: Vec<String> = line.split('\t').map(String::from).collect();
    resolve_targets(&parts)
}

fn resolve_target(target: Option<&str>) -> Result<Startup> {
    let mut s = Startup::default();
    let Some(target) = target else { return Ok(s) };
    if target.starts_with("chorong://") {
        return Startup::from_link(target);
    }
    let path = std::path::absolute(target)?;
    if path.is_dir() {
        s.root = Some(path);
    } else if path.is_file() {
        s.root = path.parent().map(PathBuf::from);
        s.file = Some(path);
    } else {
        bail!("{} does not exist", path.display());
    }
    Ok(s)
}

fn load_theme(args: &Args) -> Result<Theme> {
    match &args.theme {
        Some(file) => {
            let json = cho_core::fs::read_source_text(file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let family = ThemeFamily::parse(&json)?;
            Ok(family.theme(args.theme_name.as_deref())?.clone())
        }
        None => Ok(Theme::bundled(if args.light {
            Appearance::Light
        } else {
            Appearance::Dark
        })),
    }
}

/// Where links go to be followed on the main thread: OS hand-offs, later launches, and
/// windows opening a link elsewhere ("Open worktree").
pub struct LinkSender(pub futures::channel::mpsc::UnboundedSender<String>);

impl gpui::Global for LinkSender {}

/// Asks the app to follow `url` as if it had been clicked.
pub fn send_link(url: String, cx: &App) {
    if let Some(s) = cx.try_global::<LinkSender>() {
        s.0.unbounded_send(url).ok();
    }
}

/// What every window is opened with (from the command line).
#[derive(Clone)]
struct Settings {
    theme: Theme,
    editor: Editor,
    /// `--font`, or empty for the `appearance.code_font` setting.
    font: String,
    fold_by_default: bool,
}

/// The command-line settings every window is opened with, for windows opened from a window.
struct WindowDefaults(Settings);

impl gpui::Global for WindowDefaults {}

gpui::actions!(chorong, [Quit, MinimizeWindow, ZoomWindow]);

/// The menu bar: a Chorong menu, an Edit menu and a Window menu. macOS lists the open windows, by title, in
/// the Window menu and in the Dock icon's menu, which it builds from the Window menu, so
/// several Chorong windows can be told apart there.
fn install_menus(cx: &mut App) {
    use gpui::{Menu, MenuItem, SystemMenuType};
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &MinimizeWindow, cx| {
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &ZoomWindow, cx| {
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, _| window.zoom_window());
        }
    });
    if cfg!(target_os = "macos") {
        cx.set_menus(vec![
            Menu {
                name: "Chorong".into(),
                items: vec![
                    MenuItem::action("Settings\u{2026}", view::OpenSettings),
                    MenuItem::separator(),
                    MenuItem::os_submenu("Services", SystemMenuType::Services),
                    MenuItem::separator(),
                    MenuItem::action("Quit Chorong", Quit),
                ],
            },
            // The code view's own Copy, Copy with Location and Select All: enabled while the
            // code has the keyboard, where their keys are bound.
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::action("Copy", view::CopySelection),
                    MenuItem::action("Copy with Location", view::CopyWithLocation),
                    MenuItem::separator(),
                    MenuItem::action("Select All", view::SelectAll),
                ],
            },
            Menu {
                name: "Window".into(),
                items: vec![
                    MenuItem::action("Minimize", MinimizeWindow),
                    MenuItem::action("Zoom", ZoomWindow),
                ],
            },
        ]);
    }
}

/// Opens a window with the settings the first one got.
fn open_window_with_defaults(startup: Startup, cx: &mut App) {
    if let Some(d) = cx.try_global::<WindowDefaults>() {
        let settings = d.0.clone();
        open_window(&settings, startup, cx);
    }
}

/// "NAME — repo — Chorong" for a named window; "repo — 3 files — Chorong" for files opened together;
/// otherwise "repo — Chorong".
fn window_title(startup: &Startup) -> String {
    let repo = startup
        .root
        .as_ref()
        .and_then(|r| r.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    let files = startup.file.iter().count() + startup.extra_files.len();
    let mut parts: Vec<String> = Vec::new();
    if let Some(name) = startup
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        parts.push(name.to_string());
    }
    parts.extend(repo);
    if startup.name.is_none() && files > 1 {
        parts.push(format!("{files} files"));
    }
    parts.push("Chorong".into());
    parts.join(" — ")
}

fn open_window(settings: &Settings, startup: Startup, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1180.), px(780.)), cx);
    let title = window_title(&startup);
    let s = settings.clone();
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| {
                view::Chorong::new(
                    s.theme,
                    s.editor,
                    s.font,
                    s.fold_by_default,
                    startup,
                    window,
                    cx,
                )
            });
            let focus = view.read(cx).focus_handle();
            window.focus(&focus);
            view
        },
    );
    if let Err(e) = opened {
        eprintln!("chorong-app: cannot open a window: {e}");
    }
}

/// The folder a link opens, when the link came with one Chorong has never opened (not in the
/// recent folders). Remote (`github:` ...) links only ever resolve to known folders.
fn unfamiliar_link_folder(startup: &Startup) -> Option<PathBuf> {
    if !matches!(startup.origin, Origin::Link(_) | Origin::Investigation(_)) {
        return None;
    }
    let folder = startup.root.as_ref()?.canonicalize().ok()?;
    let known = cho_sources::recent::list()
        .iter()
        .any(|r| r.canonicalize().ok().as_ref() == Some(&folder));
    (!known).then_some(folder)
}

/// A link from the OS (macOS) or a later launch (Linux): it goes to the window that already
/// has its repository open, which comes forward and records the jump in its history.
/// Otherwise it opens a new window. A link to a repository not on this machine shows its
/// banner in the front window.
fn follow_link(url: &str, settings: &Settings, cx: &mut App) {
    let windows: Vec<WindowHandle<view::Chorong>> = cx
        .windows()
        .iter()
        .filter_map(|w| w.downcast::<view::Chorong>())
        .collect();
    let startup = match resolve_handover(url) {
        Ok(s) => s,
        Err(e) => {
            if let Some(w) = windows.first() {
                w.update(cx, |view, window, cx| {
                    window.activate_window();
                    view.show_notice(format!("{url}: {e}"), cx);
                })
                .ok();
            } else {
                eprintln!("chorong-app: {e}");
            }
            return;
        }
    };
    // A pull request Chorong has not fetched: ask first, in the front window (a link never
    // fetches by itself).
    if let Origin::AskPr(ask) = &startup.origin
        && let Some(w) = windows.first()
    {
        w.update(cx, |view, window, cx| {
            window.activate_window();
            view.ask_open_pull_request(ask.clone(), cx);
        })
        .ok();
        return;
    }
    let root = startup.root.as_ref().and_then(|r| r.canonicalize().ok());
    // A link from outside (web page, chat, another program) may name any folder on this
    // machine; one Chorong has never opened is only opened after the reader says so.
    if let Some(folder) = unfamiliar_link_folder(&startup)
        && let Some(w) = windows.first()
    {
        let url = url.to_string();
        w.update(cx, |view, window, cx| {
            window.activate_window();
            view.ask_open_link(url, folder, cx);
        })
        .ok();
        return;
    }
    let target = windows
        .iter()
        .find(|w| {
            root.is_some()
                && w.read(cx)
                    .is_ok_and(|v| v.root().and_then(|r| r.canonicalize().ok()) == root)
        })
        .or_else(|| windows.first().filter(|_| startup.root.is_none()));
    match target {
        Some(w) => {
            w.update(cx, |view, window, cx| {
                window.activate_window();
                view.follow(startup, cx);
            })
            .ok();
        }
        None => {
            // A window opened with nothing in it (macOS launches the app before handing it
            // the link) gives way to the new one.
            let empty: Vec<_> = windows
                .iter()
                .filter(|w| w.read(cx).is_ok_and(|v| v.root().is_none()))
                .copied()
                .collect();
            open_window(settings, startup, cx);
            for w in empty {
                w.update(cx, |_, window, _| window.remove_window()).ok();
            }
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    // One running viewer: on Linux the targets go to the viewer that is already running, so
    // `cho PATH…` and `chorong-app PATH…` reuse it instead of starting another. One link or
    // path, or several paths joined by tabs (a path holding a tab or a line break never goes
    // over the socket: that launch opens its own window).
    #[cfg(target_os = "linux")]
    if !args.targets.is_empty() && args.name.is_none() && args.compare.is_none() {
        let handoff = match args.targets.as_slice() {
            [link] if link.starts_with("chorong://") => Some(link.clone()),
            many => many
                .iter()
                .map(|t| {
                    let path = std::path::absolute(t).ok()?;
                    let text = path.to_string_lossy().into_owned();
                    (path.exists() && !text.contains(['\t', '\n', '\r'])).then_some(text)
                })
                .collect::<Option<Vec<_>>>()
                .map(|paths| paths.join("\t")),
        };
        if let Some(handoff) = handoff
            && instance::hand_over(&handoff)
        {
            return Ok(());
        }
    }
    let theme = load_theme(&args)?;
    let mut startup = resolve_targets(&args.targets)?;
    startup.name = args.name.clone();
    startup.compare = args.compare.clone();
    // A link to a folder Chorong has never opened starts with an empty window that asks first
    // (the scheme handler launches `chorong-app LINK`, so this is how browser links arrive).
    let mut ask_first = None;
    if unfamiliar_link_folder(&startup).is_some() {
        ask_first = args.targets.first().cloned();
        startup = Startup::default();
    }
    let editor = Editor::builtins()
        .into_iter()
        .find(|e| e.program() == args.editor)
        .with_context(|| format!("unknown editor `{}`", args.editor))?;
    let settings = Settings {
        theme,
        editor,
        font: args.font.clone().unwrap_or_default(),
        fold_by_default: !args.no_fold,
    };

    // Links arrive from the OS (macOS: the app is the `cho` scheme handler) and, on Linux,
    // from later launches over the local socket. Both are handled on the main thread.
    let (links_tx, mut links_rx) = futures::channel::mpsc::unbounded::<String>();
    #[cfg(target_os = "linux")]
    {
        let tx = links_tx.clone();
        // Fails when another viewer listens already (this one was opened with a path) or
        // there is no runtime directory; links still work on the command line.
        instance::listen(move |l| {
            tx.unbounded_send(l).ok();
        })
        .ok();
    }
    // The layout shell's icons (Lucide, bundled in cho-layout-ui) and the viewer's own.
    let app = Application::new().with_assets(assets::Assets);
    {
        let tx = links_tx.clone();
        app.on_open_urls(move |urls| {
            for u in urls {
                tx.unbounded_send(u).ok();
            }
        });
    }
    app.run(move |cx: &mut App| {
        // The bundled fonts; each window then applies the font settings.
        cho_fonts::install(cx, None, None);
        keymap::bind_defaults(cx);
        cx.set_global(LinkSender(links_tx.clone()));
        cx.set_global(WindowDefaults(settings.clone()));
        install_menus(cx);
        open_window(&settings, startup, cx);
        if let Some(url) = ask_first.take() {
            send_link(url, cx);
        }
        let settings = settings.clone();
        cx.spawn(async move |cx| {
            while let Some(url) = links_rx.next().await {
                cx.update(|cx| follow_link(&url, &settings, cx)).ok();
            }
        })
        .detach();
        cx.on_window_closed(|cx| {
            // The settings window does not keep the app alive on its own.
            if !cx
                .windows()
                .iter()
                .any(|w| w.downcast::<view::Chorong>().is_some())
            {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cho-targets-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        for d in ["repo/.git", "repo/src/a", "repo/docs", "loose/x", "loose/y"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in [
            "repo/src/a/one.rs",
            "repo/docs/two.md",
            "loose/x/1.txt",
            "loose/y/2.txt",
        ] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        dir
    }

    fn s(p: PathBuf) -> String {
        p.display().to_string()
    }

    #[test]
    fn several_files_share_their_repository_as_root() {
        let dir = tree("repo");
        let (one, two) = (dir.join("repo/src/a/one.rs"), dir.join("repo/docs/two.md"));
        let st = resolve_targets(&[s(one.clone()), s(two.clone()), s(one.clone())]).unwrap();
        assert_eq!(st.root, Some(dir.join("repo")), "the folder with .git");
        assert_eq!(st.file, Some(one));
        assert_eq!(st.extra_files, [two], "duplicates dropped");
        // No repository: their shared folder.
        let st =
            resolve_targets(&[s(dir.join("loose/x/1.txt")), s(dir.join("loose/y/2.txt"))]).unwrap();
        assert_eq!(st.root, Some(dir.join("loose")));
        // A tab-joined hand-over line is the same list.
        let line = format!(
            "{}\t{}",
            s(dir.join("loose/x/1.txt")),
            s(dir.join("loose/y/2.txt"))
        );
        assert_eq!(resolve_handover(&line).unwrap().extra_files.len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_title_names_the_window_then_the_repository() {
        let st = |name: Option<&str>, extra: usize| Startup {
            root: Some(PathBuf::from("/code/chorong")),
            file: Some(PathBuf::from("/code/chorong/a.rs")),
            extra_files: vec![PathBuf::from("/code/chorong/b.rs"); extra],
            name: name.map(String::from),
            ..Startup::default()
        };
        assert_eq!(window_title(&st(None, 0)), "chorong — Chorong");
        assert_eq!(window_title(&st(None, 2)), "chorong — 3 files — Chorong");
        assert_eq!(
            window_title(&st(Some("PR 412 review"), 2)),
            "PR 412 review — chorong — Chorong"
        );
        assert_eq!(window_title(&st(Some("  "), 0)), "chorong — Chorong");
        assert_eq!(window_title(&Startup::default()), "Chorong");
    }

    #[test]
    fn folders_links_and_missing_files_do_not_mix() {
        let dir = tree("mix");
        let one = s(dir.join("repo/src/a/one.rs"));
        assert!(resolve_targets(&[one.clone(), s(dir.join("repo/docs"))]).is_err());
        assert!(resolve_targets(&[one.clone(), "chorong://open?repo=local:/x".into()]).is_err());
        assert!(resolve_targets(&[one.clone(), s(dir.join("repo/nope.rs"))]).is_err());
        // One target behaves as before: a file opens with its folder as the root.
        let st = resolve_targets(&[one]).unwrap();
        assert_eq!(st.root, Some(dir.join("repo/src/a")));
        assert!(st.extra_files.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
