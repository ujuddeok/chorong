//! The file tree's model, independent of GPUI: folders built from the flat file list, the rows
//! visible for a set of expanded folders, compact folder chains and the icon for each file
//! (docs/public/visual-design.md, "File tree" and "Icons").

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One folder or file. Node 0 is the root folder, which has no row of its own.
#[derive(Debug)]
struct Node {
    name: String,
    /// Path relative to the root.
    rel: PathBuf,
    /// Children in display order (folders first, then files, each case-insensitively
    /// alphabetical); `None` for a file.
    children: Option<Vec<usize>>,
}

/// One visible row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Name segments: one for a file or a plain folder, several for a compact chain
    /// (`fuzz / fuzz_targets`).
    pub segments: Vec<String>,
    /// The path the row stands for (for a chain, its last folder), relative to the root.
    pub rel: PathBuf,
    pub depth: usize,
    pub kind: RowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    File,
    Folder { expanded: bool },
}

#[derive(Debug, Default)]
pub struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    /// Builds the folder structure of `files` (paths relative to the root).
    pub fn new(files: &[PathBuf]) -> Self {
        let mut nodes = vec![Node {
            name: String::new(),
            rel: PathBuf::new(),
            children: Some(Vec::new()),
        }];
        // Folder path -> node, for the folders made so far.
        let mut folders: std::collections::HashMap<PathBuf, usize> = Default::default();
        for file in files {
            let mut parent = 0;
            let mut rel = PathBuf::new();
            let comps: Vec<_> = file.components().collect();
            for (i, comp) in comps.iter().enumerate() {
                rel.push(comp);
                let name = comp.as_os_str().to_string_lossy().into_owned();
                let is_file = i + 1 == comps.len();
                if !is_file && let Some(&id) = folders.get(&rel) {
                    parent = id;
                    continue;
                }
                let id = nodes.len();
                nodes.push(Node {
                    name,
                    rel: rel.clone(),
                    children: (!is_file).then(Vec::new),
                });
                if let Some(c) = nodes[parent].children.as_mut() {
                    c.push(id);
                }
                if !is_file {
                    folders.insert(rel.clone(), id);
                }
                parent = id;
            }
        }
        // Display order, decided once.
        for i in 0..nodes.len() {
            let Some(mut children) = nodes[i].children.take() else {
                continue;
            };
            children.sort_by_cached_key(|&c| {
                (
                    nodes[c].children.is_none(),
                    nodes[c].name.to_lowercase(),
                    nodes[c].name.clone(),
                )
            });
            nodes[i].children = Some(children);
        }
        Self { nodes }
    }

    /// The visible rows, depth first, for the folders in `expanded`.
    pub fn rows(&self, expanded: &HashSet<PathBuf>) -> Vec<Row> {
        let mut out = Vec::new();
        if self.nodes.is_empty() {
            return out;
        }
        self.push_children(0, 0, expanded, &mut out);
        out
    }

    fn push_children(
        &self,
        node: usize,
        depth: usize,
        expanded: &HashSet<PathBuf>,
        out: &mut Vec<Row>,
    ) {
        let Some(children) = &self.nodes[node].children else {
            return;
        };
        for &c in children {
            let n = &self.nodes[c];
            if n.children.is_none() {
                out.push(Row {
                    segments: vec![n.name.clone()],
                    rel: n.rel.clone(),
                    depth,
                    kind: RowKind::File,
                });
                continue;
            }
            // Compact folders: follow a chain of folders that each hold only one folder.
            let (mut last, mut segments) = (c, vec![n.name.clone()]);
            while let Some([only]) = self.nodes[last].children.as_deref()
                && self.nodes[*only].children.is_some()
            {
                last = *only;
                segments.push(self.nodes[last].name.clone());
            }
            let rel = self.nodes[last].rel.clone();
            let open = expanded.contains(&rel);
            out.push(Row {
                segments,
                rel,
                depth,
                kind: RowKind::Folder { expanded: open },
            });
            if open {
                self.push_children(last, depth + 1, expanded, out);
            }
        }
    }
}

/// Adds every ancestor folder of `rel` to `expanded`, so the file's row is visible.
pub fn reveal(rel: &Path, expanded: &mut HashSet<PathBuf>) {
    let mut p = rel.parent();
    while let Some(dir) = p.filter(|d| !d.as_os_str().is_empty()) {
        expanded.insert(dir.to_path_buf());
        p = dir.parent();
    }
}

/// The rows the open file's parent folder holds, and that folder's depth: the indent guide
/// at that depth is drawn in `panel.indent_guide_active` across these rows.
pub fn active_guide(rows: &[Row], current: &Path) -> Option<(std::ops::Range<usize>, usize)> {
    let at = rows.iter().position(|r| r.rel == current)?;
    let depth = rows[at].depth.checked_sub(1)?;
    let start = rows[..at].iter().rposition(|r| r.depth == depth)? + 1;
    let end = rows[at..]
        .iter()
        .position(|r| r.depth <= depth)
        .map_or(rows.len(), |n| at + n);
    Some((start..end, depth))
}

/// The Lucide icon for a file, by type, one colour (docs/public/visual-design.md, "Icons").
pub fn file_icon(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    if ext == "lock"
        || matches!(
            lower.as_str(),
            "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "npm-shrinkwrap.json"
        )
    {
        return "lock";
    }
    match ext {
        "json" | "jsonc" | "json5" | "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "plist"
        | "xml" => "file-json",
        "md" | "markdown" | "txt" | "rst" | "adoc" | "org" => "file-text",
        "rs" | "swift" | "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" | "py"
        | "pyi" | "c" | "h" | "cc" | "cpp" | "hpp" | "m" | "mm" | "go" | "java" | "kt" | "rb"
        | "sh" | "bash" | "zsh" | "fish" | "css" | "scss" | "html" | "vue" | "svelte" | "scm"
        | "sql" | "lua" | "zig" | "cs" | "php" | "ex" | "exs" => "file-code",
        _ if lower.starts_with("license")
            || lower.starts_with("readme")
            || lower.starts_with("changelog")
            || lower.starts_with("notice") =>
        {
            "file-text"
        }
        _ if matches!(lower.as_str(), "makefile" | "dockerfile" | "justfile") => "file-code",
        _ => "file",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(paths: &[&str]) -> Tree {
        let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        Tree::new(&files)
    }

    fn labels(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                let mark = match r.kind {
                    RowKind::File => "",
                    RowKind::Folder { expanded: true } => "v ",
                    RowKind::Folder { expanded: false } => "> ",
                };
                format!("{}{mark}{}", "  ".repeat(r.depth), r.segments.join(" / "))
            })
            .collect()
    }

    fn set(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn folders_first_then_files_case_insensitive() {
        let t = tree(&[
            "b.rs",
            "A.md",
            "src/x.rs",
            "Docs/a.md",
            "Docs/b.md",
            "zeta/q.rs",
            "zeta/r.rs",
        ]);
        assert_eq!(
            labels(&t.rows(&set(&[]))),
            ["> Docs", "> src", "> zeta", "A.md", "b.rs"]
        );
    }

    #[test]
    fn expanded_folders_show_their_children_indented() {
        let t = tree(&["src/main.rs", "src/view/a.rs", "src/view/b.rs", "README.md"]);
        assert_eq!(
            labels(&t.rows(&set(&["src", "src/view"]))),
            [
                "v src",
                "  v view",
                "    a.rs",
                "    b.rs",
                "  main.rs",
                "README.md"
            ]
        );
    }

    #[test]
    fn compact_folders_join_single_folder_chains() {
        let t = tree(&["fuzz/fuzz_targets/a.rs", "fuzz/fuzz_targets/b.rs", "x.rs"]);
        let rows = t.rows(&set(&["fuzz/fuzz_targets"]));
        assert_eq!(
            labels(&rows),
            ["v fuzz / fuzz_targets", "  a.rs", "  b.rs", "x.rs"]
        );
        assert_eq!(rows[0].rel, PathBuf::from("fuzz/fuzz_targets"));
        // A folder that also holds a file is not compacted.
        let t = tree(&["a/b/c.rs", "a/d.rs"]);
        assert_eq!(labels(&t.rows(&set(&["a"]))), ["v a", "  > b", "  d.rs"]);
    }

    #[test]
    fn reveal_expands_every_ancestor() {
        let t = tree(&[
            "crates/core/src/buffer.rs",
            "crates/core/src/line.rs",
            "crates/ui/a.rs",
        ]);
        let mut open = HashSet::new();
        reveal(Path::new("crates/core/src/buffer.rs"), &mut open);
        assert_eq!(open, set(&["crates", "crates/core", "crates/core/src"]));
        assert_eq!(
            labels(&t.rows(&open)),
            [
                "v crates",
                "  v core / src",
                "    buffer.rs",
                "    line.rs",
                "  > ui"
            ]
        );
    }

    #[test]
    fn active_guide_spans_the_parent_folders_rows() {
        let t = tree(&["a/x.rs", "a/y/z.rs", "a/y/w.rs", "b.rs"]);
        let rows = t.rows(&set(&["a", "a/y"]));
        // v a, > y (expanded), w, z, x, b
        assert_eq!(
            labels(&rows),
            ["v a", "  v y", "    w.rs", "    z.rs", "  x.rs", "b.rs"]
        );
        assert_eq!(active_guide(&rows, Path::new("a/y/z.rs")), Some((2..4, 1)));
        assert_eq!(active_guide(&rows, Path::new("a/x.rs")), Some((1..5, 0)));
        // A file at the root has no guide; an unknown file neither.
        assert_eq!(active_guide(&rows, Path::new("b.rs")), None);
        assert_eq!(active_guide(&rows, Path::new("nope.rs")), None);
    }

    #[test]
    fn file_icons_by_type() {
        assert_eq!(file_icon("buffer.rs"), "file-code");
        assert_eq!(file_icon("App.tsx"), "file-code");
        assert_eq!(file_icon("Cargo.toml"), "file-json");
        assert_eq!(file_icon("package.json"), "file-json");
        assert_eq!(file_icon("tsconfig.json"), "file-json");
        assert_eq!(file_icon("Cargo.lock"), "lock");
        assert_eq!(file_icon("package-lock.json"), "lock");
        assert_eq!(file_icon("README.md"), "file-text");
        assert_eq!(file_icon("LICENSE"), "file-text");
        assert_eq!(file_icon("image.png"), "file");
    }
}
