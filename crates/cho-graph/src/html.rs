//! The HTML page. The data, the page script and the theme colours are always inside the file.
//! ELK.js and Cytoscape.js are either inlined (a self-contained page, for sharing) or loaded
//! from one shared copy in Chorong's cache folder by a relative path. Nothing is fetched from the
//! network.

use crate::model::{EdgeKind, Graph};
use cho_theme::{Appearance, Rgba, Theme};
use std::path::{Path, PathBuf};

const CYTOSCAPE: &str = include_str!("../assets/cytoscape.min.js");
const ELK: &str = include_str!("../assets/elk.bundled.js");
const GRAPH_JS: &str = include_str!("../assets/graph.js");
const GRAPH_CSS: &str = include_str!("../assets/graph.css");
const CYTOSCAPE_LICENSE: &str = include_str!("../assets/LICENSE-cytoscape-MIT.txt");
const ELK_LICENSE: &str = include_str!("../assets/LICENSE-elkjs-EPL-2.0.md");

/// Vendored library versions, pinned. Also named in `docs/public/graph.md` (NOTICE).
pub const CYTOSCAPE_VERSION: &str = "3.30.2";
pub const ELK_VERSION: &str = "0.9.3";

#[derive(Debug, Clone, Default)]
pub struct PageOptions {
    /// Forces a theme; `None` follows the system.
    pub theme: Option<Appearance>,
    /// Where the page gets ELK.js and Cytoscape.js.
    pub assets: Assets,
}

/// Where a page gets its two libraries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Assets {
    /// Inside the page: about 2 MB, works anywhere (pages for sharing).
    #[default]
    Inline,
    /// From the shared copy, by this relative URL prefix (ends in `/`).
    Shared(String),
}

/// The folder name of the shared copy, by library versions, so a new Chorong with new versions
/// never loads old files.
pub(crate) fn assets_folder_name() -> String {
    format!("graph-assets/cytoscape-{CYTOSCAPE_VERSION}-elk-{ELK_VERSION}")
}

/// Writes the shared copy of the libraries and their licence texts under `cache` (Chorong's cache
/// folder) unless it is already there, and returns its directory. A file whose content differs
/// (an older or damaged copy) is replaced.
#[allow(clippy::disallowed_methods)] // Chorong's own cache folder
pub(crate) fn install_shared_assets(cache: &Path) -> std::io::Result<PathBuf> {
    let dir = cache.join(assets_folder_name());
    std::fs::create_dir_all(&dir)?;
    for (name, body) in [
        ("cytoscape.min.js", CYTOSCAPE),
        ("elk.bundled.js", ELK),
        ("LICENSE-cytoscape-MIT.txt", CYTOSCAPE_LICENSE),
        ("LICENSE-elkjs-EPL-2.0.md", ELK_LICENSE),
    ] {
        let path = dir.join(name);
        let same = std::fs::metadata(&path).is_ok_and(|m| m.len() == body.len() as u64)
            && std::fs::read(&path).is_ok_and(|b| b == body.as_bytes());
        if !same {
            // Write beside, then rename: a page opened meanwhile never sees half a file. The
            // name is per write (process and counter), so two threads never share a temp file.
            static WRITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let tmp = dir.join(format!(
                ".{name}.{}.{}",
                std::process::id(),
                WRITE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::write(&tmp, body)?;
            std::fs::rename(&tmp, &path)?;
        }
    }
    Ok(dir)
}

/// The assets for a page written to `page`, loading from `assets_dir` by a relative path.
/// `None` when no relative path exists (different drives), so the caller inlines instead.
/// A relative path keeps the home folder out of the page.
pub(crate) fn shared_assets_for(page: &Path, assets_dir: &Path) -> Option<Assets> {
    let from = std::path::absolute(page.parent()?).ok()?;
    let to = std::path::absolute(assets_dir).ok()?;
    let a: Vec<_> = from.components().collect();
    let b: Vec<_> = to.components().collect();
    if a.first() != b.first() {
        return None;
    }
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); a.len() - common];
    for c in &b[common..] {
        parts.push(c.as_os_str().to_str()?.to_string());
    }
    let mut url = String::new();
    for p in parts {
        for ch in p.chars() {
            match ch {
                ' ' => url.push_str("%20"),
                '#' => url.push_str("%23"),
                '?' => url.push_str("%3F"),
                '%' => url.push_str("%25"),
                '"' => url.push_str("%22"),
                c => url.push(c),
            }
        }
        url.push('/');
    }
    Some(Assets::Shared(url))
}

fn css(c: Rgba) -> String {
    if c.a == 0xff {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("rgba({},{},{},{:.2})", c.r, c.g, c.b, c.a as f32 / 255.0)
    }
}

/// The CSS custom properties for one theme: surfaces from the editor, edge kinds from the
/// syntax colours, so the page matches the viewer.
fn theme_vars(t: &Theme) -> String {
    let syn = |keys: &[&str]| -> Rgba {
        keys.iter()
            .find_map(|k| t.syntax_style(k).and_then(|s| s.color))
            .unwrap_or(t.text_muted)
    };
    let accent = t.players.first().map(|p| p.cursor).unwrap_or(t.text);
    let kinds = [
        (EdgeKind::Calls, syn(&["function"])),
        (EdgeKind::Constructs, syn(&["constructor", "type"])),
        (EdgeKind::Renders, syn(&["tag", "attribute"])),
        (EdgeKind::TypeUses, syn(&["type"])),
        (EdgeKind::Implements, syn(&["keyword"])),
        (EdgeKind::ReExports, syn(&["namespace", "string"])),
        (EdgeKind::References, t.text_muted),
    ];
    let mut s = format!(
        "--bg:{};--fg:{};--muted:{};--border:{};--panel:{};--sel:{};--accent:{};--cycle:{};",
        css(t.editor_background),
        css(t.editor_foreground),
        css(t.text_muted),
        css(t.border),
        css(t.panel_background),
        css(t.element_selected),
        css(accent),
        css(syn(&["string", "constant"])),
    );
    for (k, c) in kinds {
        s.push_str(&format!("--k-{}:{};", k.as_str(), css(c)));
    }
    s
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A script-safe JSON string: no `</script>`, no line separators.
fn json_for_script(g: &Graph) -> String {
    serde_json::to_string(g)
        .expect("graph serialises")
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

pub fn to_html(g: &Graph, opts: &PageOptions) -> String {
    let dark = Theme::bundled(Appearance::Dark);
    let light = Theme::bundled(Appearance::Light);
    let forced = match opts.theme {
        Some(Appearance::Dark) => " data-theme=\"dark\"",
        Some(Appearance::Light) => " data-theme=\"light\"",
        None => "",
    };
    let m = &g.meta.provenance;
    let dirty = if m.dirty {
        "<b class=\"dirty\">uncommitted changes</b>".to_string()
    } else {
        "<b>clean</b>".to_string()
    };
    // A page exported for sharing names only the commit: no worktree, branch or dirty state.
    let place = if m.shared {
        format!(
            "<span>commit <b>{}</b></span><span>exported for sharing</span>",
            esc(m.commit.as_deref().unwrap_or("(none)")),
        )
    } else {
        format!(
            "<span>worktree <b>{}</b></span><span>branch <b>{}</b></span><span>commit <b>{}</b></span><span>working tree {}</span>",
            esc(&m.worktree),
            esc(m.branch.as_deref().unwrap_or("(detached)")),
            esc(m.commit.as_deref().unwrap_or("(none)")),
            dirty,
        )
    };
    let mut meta = format!(
        "{place}<span>index <b>{}</b></span><span>{} · direction {} · depth {}</span>",
        esc(g.meta.index_status.as_str()),
        esc(g.view.kind.as_str()),
        esc(g.view.direction.as_str()),
        g.view
            .depth
            .map(|d| d.to_string())
            .unwrap_or_else(|| "-".into()),
    );
    if !g.reachable_tests.is_empty() {
        meta.push_str(&format!(
            "<span>tests reaching the change <b>{}</b></span>",
            g.reachable_tests.len()
        ));
    }
    let notes = if g.notes.is_empty() {
        String::new()
    } else {
        format!(
            "<ul class=\"notes\">{}</ul>",
            g.notes
                .iter()
                .map(|n| format!("<li>{}</li>", esc(n)))
                .collect::<String>()
        )
    };
    let tests = if g.reachable_tests.is_empty() {
        String::new()
    } else {
        format!(
            "<ul class=\"notes\">Test files that reference the changed code (reachability, not coverage): {}</ul>",
            g.reachable_tests
                .iter()
                .map(|t| format!("<li>{}</li>", esc(t)))
                .collect::<String>()
        )
    };
    let (libs, notice) = match &opts.assets {
        Assets::Inline => (
            format!(
                "<!-- Cytoscape.js {CYTOSCAPE_VERSION} (MIT) and ELK.js {ELK_VERSION} (EPL-2.0), unmodified; licence texts are in the comments at the end of this file and in crates/cho-graph/assets. -->\n<script>{}</script>\n<script>{}</script>\n",
                CYTOSCAPE.replace("</script", "<\\/script"),
                ELK.replace("</script", "<\\/script"),
            ),
            format!(
                "<!--\nThird-party software in this page\n\nCytoscape.js {CYTOSCAPE_VERSION}: MIT\n{}\n\nELK.js {ELK_VERSION} (Eclipse Layout Kernel, GWT-compiled): EPL-2.0. Source: https://github.com/kieler/elkjs\n{}\n-->\n",
                CYTOSCAPE_LICENSE.replace("-->", "--&gt;"),
                ELK_LICENSE.replace("-->", "--&gt;"),
            ),
        ),
        Assets::Shared(prefix) => {
            let p = esc(prefix);
            (
                format!(
                    "<!-- Cytoscape.js {CYTOSCAPE_VERSION} (MIT) and ELK.js {ELK_VERSION} (EPL-2.0), unmodified, loaded from Chorong's shared copy; their licence texts are beside them. -->\n<script src=\"{p}cytoscape.min.js\"></script>\n<script src=\"{p}elk.bundled.js\"></script>\n"
                ),
                format!(
                    "<!--\nThird-party software used by this page, loaded from {p}: Cytoscape.js {CYTOSCAPE_VERSION} (MIT, {p}LICENSE-cytoscape-MIT.txt) and ELK.js {ELK_VERSION} (Eclipse Layout Kernel, EPL-2.0, {p}LICENSE-elkjs-EPL-2.0.md; source: https://github.com/kieler/elkjs).\n-->\n"
                ),
            )
        }
    };
    format!(
        "<!doctype html>\n<html lang=\"en\"{forced}>\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{title} · Chorong graph</title>\n<style>\n:root{{{dark_vars}color-scheme:dark}}\n@media (prefers-color-scheme: light){{:root:not([data-theme]){{{light_vars}color-scheme:light}}}}\n:root[data-theme=\"dark\"]{{{dark_vars}color-scheme:dark}}\n:root[data-theme=\"light\"]{{{light_vars}color-scheme:light}}\n{css}\n</style>\n</head>\n<body>\n<header>\n<h1>{title}</h1>\n<div class=\"meta\">{meta}</div>\n<div class=\"limits\">{limits}</div>\n{notes}{tests}\n</header>\n<div class=\"bar\" id=\"bar\"></div>\n<main><div id=\"cy\"></div><aside id=\"side\"></aside></main>\n{libs}<script id=\"cho-data\" type=\"application/json\">{data}</script>\n<script>{js}</script>\n{notice}</body>\n</html>\n",
        title = esc(&g.view.title),
        dark_vars = theme_vars(&dark),
        light_vars = theme_vars(&light),
        css = GRAPH_CSS,
        meta = meta,
        limits = esc(g.meta.limits),
        libs = libs,
        data = json_for_script(g),
        js = GRAPH_JS,
        notice = notice,
    )
}

/// Chorong's cache folder (the parent of the index cache), where the shared copy lives.
fn cache_folder() -> Option<PathBuf> {
    let cache = cho_semantic::IndexCache::default_location()?;
    let base = std::path::absolute(cache.base()).ok()?;
    Some(base.parent().unwrap_or(&base).to_path_buf())
}

/// The assets for a page about to be written to `page`: inline when `self_contained` (pages
/// for sharing), otherwise a shared copy in the page's own folder, installing it if needed.
///
/// Safari lets a `file://` page load scripts only from its own folder or below, so the copy
/// sits under the page, never up a `..`. Only a page inside Chorong's cache folder gets one (Chorong
/// never adds folders beside a page written elsewhere); any other page, or one with no cache
/// folder, is inlined.
pub fn assets_for_page(page: &Path, self_contained: bool) -> Assets {
    if self_contained {
        return Assets::Inline;
    }
    let dir = page.parent().and_then(|d| std::path::absolute(d).ok());
    match (cache_folder(), dir) {
        (Some(cache), Some(dir)) if dir.starts_with(&cache) => shared_beside(page, &dir),
        _ => Assets::Inline,
    }
}

/// The shared copy installed in `dir` (the page's folder), loaded by a path that only goes down.
pub(crate) fn shared_beside(page: &Path, dir: &Path) -> Assets {
    install_shared_assets(dir)
        .ok()
        .and_then(|assets| shared_assets_for(page, &assets))
        .filter(|a| !matches!(a, Assets::Shared(p) if p.starts_with("..")))
        .unwrap_or(Assets::Inline)
}
