//! The investigation skill: its text (built into the binary), the version stamp written into
//! an installed copy, and the note when a repository's copy is older than this cho's.

use std::path::Path;

const INVESTIGATION_SKILL: &str = include_str!("../assets/investigation-skill.md");

/// Where `--install` is usually pointed, relative to a repository root.
pub(crate) const INSTALL_DIRS: [&str; 2] = [".claude/skills", ".cursor/skills"];

/// The installed file, under an install directory.
pub(crate) const FILE: &str = "chorong-investigations/SKILL.md";

const STAMP_START: &str = "<!-- cho skill investigation ";

/// The skill text with this cho's versions filled in, before stamping.
fn rendered() -> String {
    INVESTIGATION_SKILL
        .replace("{{CHORONG_VERSION}}", env!("CARGO_PKG_VERSION"))
        .replace(
            "{{FORMAT_VERSION}}",
            &cho_investigation::FORMAT_VERSION.to_string(),
        )
}

/// A short hash of the skill text: changes whenever the text does, release or not (FNV-1a).
pub(crate) fn version() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in rendered().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:012x}", h >> 16)
}

/// The skill as printed and installed: the text with a stamp line after its front matter.
pub(crate) fn text() -> String {
    let body = rendered();
    let stamp = format!(
        "{STAMP_START}{}: written by cho {}; update with `cho skill investigation --install <dir> --force` -->\n",
        version(),
        env!("CARGO_PKG_VERSION"),
    );
    // After the closing `---` of the front matter, which skill loaders read first.
    match body.match_indices("\n---\n").next() {
        Some((at, m)) => {
            let split = at + m.len();
            format!("{}{stamp}{}", &body[..split], &body[split..])
        }
        None => format!("{stamp}{body}"),
    }
}

/// The version stamped into an installed copy, if it has one.
pub(crate) fn stamp_of(installed: &str) -> Option<&str> {
    let rest = &installed[installed.find(STAMP_START)? + STAMP_START.len()..];
    rest.split(':')
        .next()
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// Copies of the skill under `root` that differ from this cho's (older, newer, or written
/// before stamps existed), as repository-relative paths.
pub(crate) fn outdated_under(root: &Path) -> Vec<String> {
    let current = version();
    INSTALL_DIRS
        .iter()
        .map(|dir| format!("{dir}/{FILE}"))
        .filter(|rel| {
            // A repository is untrusted input: read inside its root only.
            cho_core::fs::read_source_text_in(root, Path::new(rel))
                .is_ok_and(|text| stamp_of(&text) != Some(current.as_str()))
        })
        .collect()
}

/// One line for stderr about outdated copies, or `None` when there are none.
pub(crate) fn outdated_note(paths: &[String]) -> Option<String> {
    let first = paths.first()?;
    let dir = first.trim_end_matches(FILE).trim_end_matches('/');
    Some(format!(
        "note: {} is not this cho's investigation skill; update it with `cho skill investigation --install {dir} --force`",
        paths.join(" and ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_follows_the_front_matter_and_reads_back() {
        let text = text();
        assert!(text.starts_with("---\n"), "front matter stays first");
        let stamp_at = text.find(STAMP_START).unwrap();
        let front_end = text.find("\n---\n").unwrap();
        assert!(stamp_at > front_end);
        assert_eq!(stamp_of(&text), Some(version().as_str()));
        assert_eq!(version().len(), 12);
    }

    #[test]
    fn an_unstamped_or_older_copy_is_outdated() {
        let dir = std::env::temp_dir().join(format!("cho-skill-{}", std::process::id()));
        let skills = dir.join(".claude/skills/chorong-investigations");
        std::fs::create_dir_all(&skills).unwrap();
        let path = skills.join("SKILL.md");
        #[allow(clippy::disallowed_methods)] // test fixture, written by the test
        let write = |t: &str| std::fs::write(&path, t).unwrap();
        write(&text());
        assert!(outdated_under(&dir).is_empty(), "this cho's copy");
        write("---\nname: chorong-investigations\n---\n# old\n");
        assert_eq!(
            outdated_under(&dir),
            vec![".claude/skills/chorong-investigations/SKILL.md"]
        );
        let note = outdated_note(&outdated_under(&dir)).unwrap();
        assert!(note.contains("--install .claude/skills --force"), "{note}");
        write(&text().replace(&version(), "000000000000"));
        assert_eq!(outdated_under(&dir).len(), 1, "an older stamp");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
