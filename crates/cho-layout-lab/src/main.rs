//! `cho-layout-lab`: a prototype window that renders `cho-layout` with placeholder panes.
//!
//! It exists to try docks, drag and drop, resizing, minimise, maximise, focus layouts, the tray
//! and pins on their own, with no repository loaded. The shell itself lives in `cho-layout-ui`
//! (the viewer hosts the same one); this crate hosts it with placeholder pane bodies
//! (`body.rs`), the keymap files (`keymap.rs`) and the starting scenes.

mod body;
mod keymap;
mod lab;
mod scenes;

use anyhow::{Context as _, Result};
use cho_layout_ui::shell::{Shell, ShellOptions};
use cho_theme::{Appearance, Theme, ThemeFamily};
use clap::Parser;
use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "cho-layout-lab", about = "Chorong layout prototype")]
struct Args {
    /// Which arrangement to start from.
    #[arg(long, value_enum, default_value_t = scenes::Scene::Default)]
    scene: scenes::Scene,
    /// A Zed theme file (JSON, schema v0.2.0).
    #[arg(long)]
    theme: Option<PathBuf>,
    /// Which theme in the file to use. Defaults to the first.
    #[arg(long)]
    theme_name: Option<String>,
    /// Use the bundled light theme.
    #[arg(long)]
    light: bool,
    /// Write one line per frame (start time, frame time, interval, last operation) to FILE.
    #[arg(long)]
    frame_log: Option<PathBuf>,
    /// Save the layout under DIR (off the UI thread, debounced), as the viewer will.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Start with the frame-time overlay shown.
    #[arg(long)]
    show_stats: bool,
    /// Add the optional Vim keys (h/j/k/l in Layout mode).
    #[arg(long)]
    vim: bool,
}

#[allow(clippy::disallowed_methods)] // a theme file the developer named
fn load_theme(args: &Args) -> Result<Theme> {
    match &args.theme {
        Some(file) => {
            let json = std::fs::read_to_string(file)
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

fn main() -> Result<()> {
    let args = Args::parse();
    let theme = load_theme(&args)?;
    let scene = scenes::build(args.scene)?;
    let vim = args.vim;
    let options = ShellOptions {
        frame_log: args.frame_log.clone(),
        state_dir: args.state_dir.clone(),
        state_key: "layout-lab".into(),
        show_stats: args.show_stats,
        autosave: true,
        metrics: None,
    };

    Application::new()
        .with_assets(cho_layout_ui::assets::Assets)
        .run(move |cx: &mut App| {
            // The bundled IBM Plex, as in the viewer.
            cho_layout_ui::fonts::install(cx, None, None);
            keymap::bind(cx, vim);
            let bounds = Bounds::centered(None, size(px(1180.), px(780.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Layout lab — Chorong".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    let focus = cx.focus_handle();
                    let colors =
                        cho_layout_ui::colors::Colors::new(&theme, cho_layout_ui::fonts::pick(cx));
                    let bodies = std::rc::Rc::new(lab::LabBodies { colors });
                    let shell =
                        cx.new(|cx| Shell::new(scene, &theme, options, bodies, focus.clone(), cx));
                    window.focus(&focus);
                    cx.new(|_| lab::LabView::new(shell, focus))
                },
            )
            .expect("open window");
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    Ok(())
}
