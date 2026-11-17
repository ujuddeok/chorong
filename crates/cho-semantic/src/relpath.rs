//! Repository-relative paths: `/`-separated, no leading `/`, and `""` for the top
//! (the repository root, or a package's own directory). One set of rules for every
//! module that joins, splits or compares such paths.

use std::path::{Component, Path};

/// `dir/name`, or `name` when `dir` is the top (`""`).
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The directory of a path (`""` at the top).
pub fn parent(p: &str) -> &str {
    p.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// `dir`, then each of its ancestors, ending with the top (`""`).
pub fn ancestors(dir: &str) -> impl Iterator<Item = &str> {
    let mut next = Some(dir);
    std::iter::from_fn(move || {
        let cur = next?;
        next = (!cur.is_empty()).then(|| parent(cur));
        Some(cur)
    })
}

/// A project root as recorded or given (`/web/`, `.`, ``): surrounding `/` dropped, and
/// `.` meaning the top.
pub fn trim_root(p: &str) -> &str {
    match p.trim_matches('/') {
        "." => "",
        t => t,
    }
}

/// `./a/../b/c` to `b/c`. `None` when the path leaves its base or is absolute.
pub fn normalize(p: &str) -> Option<String> {
    if p.starts_with('/') {
        return None;
    }
    let mut out: Vec<&str> = vec![];
    for s in p.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    Some(out.join("/"))
}

/// `path` relative to `dir` when `path` is `dir` itself (giving `""`) or lies under it.
/// The top (`""`) contains every path.
pub fn within<'a>(path: &'a str, dir: &str) -> Option<&'a str> {
    if dir.is_empty() {
        return Some(path);
    }
    if path == dir {
        return Some("");
    }
    path.strip_prefix(dir)?.strip_prefix('/')
}

/// `path` relative to `dir` when it lies strictly under it (never `dir` itself). The top
/// (`""`) contains every non-empty path.
pub fn below<'a>(path: &'a str, dir: &str) -> Option<&'a str> {
    within(path, dir).filter(|rest| !rest.is_empty())
}

/// Is `path` relative with only plain names (no `..`, no root, no prefix), so joining it
/// to a root can't leave the root?
pub fn is_safe_relative(path: &str) -> bool {
    Path::new(path)
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_and_splits() {
        assert_eq!(join("", "a.ts"), "a.ts");
        assert_eq!(join("web/src", "a.ts"), "web/src/a.ts");
        assert_eq!(parent("web/src/a.ts"), "web/src");
        assert_eq!(parent("a.ts"), "");
        assert_eq!(
            ancestors("a/b/c").collect::<Vec<_>>(),
            ["a/b/c", "a/b", "a", ""]
        );
        assert_eq!(ancestors("").collect::<Vec<_>>(), [""]);
        assert_eq!(trim_root("/web/"), "web");
        assert_eq!(trim_root("."), "");
    }

    #[test]
    fn within_counts_the_directory_itself_and_below_does_not() {
        assert_eq!(within("web", "web"), Some(""));
        assert_eq!(below("web", "web"), None);
        assert_eq!(within("web/a.ts", "web"), Some("a.ts"));
        assert_eq!(below("web/a.ts", "web"), Some("a.ts"));
        assert_eq!(within("webapp/a.ts", "web"), None);
        assert_eq!(below("webapp/a.ts", "web"), None);
        assert_eq!(within("a.ts", ""), Some("a.ts"));
        assert_eq!(below("a.ts", ""), Some("a.ts"));
        assert_eq!(below("", ""), None);
    }

    #[test]
    fn safe_paths_stay_inside() {
        assert!(is_safe_relative("a/b.rs"));
        assert!(!is_safe_relative("../b.rs"));
        assert!(!is_safe_relative("/etc/passwd"));
        assert_eq!(normalize("./a/../b/c").as_deref(), Some("b/c"));
        assert_eq!(normalize("../x"), None);
    }
}
