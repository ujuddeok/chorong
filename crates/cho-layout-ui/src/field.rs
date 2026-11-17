//! Editing a one-line query field (a picker's query, a search box) from keystrokes, the same
//! way in the viewer and the shell's pane pickers.

use gpui::Keystroke;

/// What a keystroke did to a query field.
#[derive(Debug, PartialEq)]
pub enum FieldEdit {
    /// Not a field key: let it through.
    None,
    /// The text changed.
    Text,
    /// Only the selection changed.
    Selection,
    /// Cmd-C on a selected query: copy this.
    Copy(String),
}

/// What a keystroke means to a one-line text field, before it is applied to any text.
///
/// This is the only code in Chorong that reads typed text from a keystroke (`key_char`): every
/// text field goes through [`field_key`], usually by way of [`edit_field`], so each one gets the
/// same keys (Cmd-A, copy, paste, word and line delete). A field that must handle its text
/// itself (a secret that is wiped as it is edited) matches on these instead of reading keys.
#[derive(Debug, PartialEq)]
pub enum FieldKey {
    /// Typed or pasted text (control characters removed): replaces a selection.
    Insert(String),
    /// Backspace: the selection, else the last character.
    Backspace,
    /// Option-Backspace (Ctrl-Backspace off macOS): the selection, else the last word.
    DeleteWord,
    /// Cmd-Backspace: the whole text.
    Clear,
    /// Cmd-A.
    SelectAll,
    /// Cmd-C (copies only a selection).
    Copy,
    /// An arrow, Home or End: drops a selection.
    Deselect,
    /// Not a text key: let it through.
    Other,
}

/// Reads `k` as a text-field key. `paste` is the clipboard's text, for Cmd-V.
pub fn field_key(k: &Keystroke, paste: Option<&str>) -> FieldKey {
    let m = &k.modifiers;
    let cmd = m.secondary();
    let word = if cfg!(target_os = "macos") {
        m.alt
    } else {
        m.control
    };
    match k.key.as_str() {
        "a" if cmd => FieldKey::SelectAll,
        "c" if cmd => FieldKey::Copy,
        "v" if cmd => {
            let text: String = paste
                .unwrap_or_default()
                .chars()
                .filter(|c| !c.is_control())
                .collect();
            if text.is_empty() {
                FieldKey::Other
            } else {
                FieldKey::Insert(text)
            }
        }
        // Off macOS the secondary modifier is Ctrl, which is also the word-delete modifier:
        // Ctrl-Backspace deletes a word there, never the whole text.
        "backspace" if cmd && !word => FieldKey::Clear,
        "backspace" if word => FieldKey::DeleteWord,
        "backspace" => FieldKey::Backspace,
        "left" | "right" | "home" | "end" => FieldKey::Deselect,
        _ => match &k.key_char {
            Some(text)
                if !m.control && !m.alt && !m.platform && text.chars().all(|c| !c.is_control()) =>
            {
                FieldKey::Insert(text.clone())
            }
            _ => FieldKey::Other,
        },
    }
}

/// Applies a keystroke to a single-line query with a select-all state (`all`): Cmd-A selects
/// the query, typing or pasting (`paste`, the clipboard text when the key is Cmd-V) replaces
/// a selection, Backspace deletes it or the last character, Cmd-Backspace clears the query,
/// Option-Backspace (Ctrl-Backspace off macOS) deletes the last word, an arrow deselects.
pub fn edit_field(
    k: &Keystroke,
    query: &mut String,
    all: &mut bool,
    paste: Option<&str>,
) -> FieldEdit {
    match field_key(k, paste) {
        FieldKey::SelectAll => {
            *all = !query.is_empty();
            FieldEdit::Selection
        }
        FieldKey::Copy if *all => FieldEdit::Copy(query.clone()),
        FieldKey::Copy | FieldKey::Other => FieldEdit::None,
        FieldKey::Insert(text) => {
            if std::mem::take(all) {
                query.clear();
            }
            query.push_str(&text);
            FieldEdit::Text
        }
        FieldKey::Clear => {
            *all = false;
            query.clear();
            FieldEdit::Text
        }
        FieldKey::Backspace | FieldKey::DeleteWord if std::mem::take(all) => {
            query.clear();
            FieldEdit::Text
        }
        FieldKey::Backspace => {
            query.pop();
            FieldEdit::Text
        }
        FieldKey::DeleteWord => {
            let kept = query.trim_end().len();
            query.truncate(kept);
            let word_start = query
                .char_indices()
                .rev()
                .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
                .last()
                .map(|(i, _)| i);
            match word_start {
                Some(i) => query.truncate(i),
                None => {
                    query.pop();
                }
            }
            FieldEdit::Text
        }
        FieldKey::Deselect if *all => {
            *all = false;
            FieldEdit::Selection
        }
        FieldKey::Deselect => FieldEdit::None,
    }
}

#[cfg(test)]
mod field_tests {
    use super::*;

    /// A keystroke as typed: `cmd` is the platform's command key (Ctrl off macOS).
    fn key(spec: &str) -> Keystroke {
        let spec = if cfg!(target_os = "macos") {
            spec.to_string()
        } else {
            spec.replace("cmd-", "ctrl-")
        };
        let mut k = Keystroke::parse(&spec).unwrap();
        if k.key.chars().count() == 1 && !k.modifiers.secondary() {
            k.key_char = Some(k.key.clone());
        }
        k
    }

    #[test]
    fn select_all_then_type_replaces_and_backspace_clears() {
        let (mut q, mut all) = ("graph".to_string(), false);
        assert_eq!(
            edit_field(&key("cmd-a"), &mut q, &mut all, None),
            FieldEdit::Selection
        );
        assert!(all);
        assert_eq!(
            edit_field(&key("x"), &mut q, &mut all, None),
            FieldEdit::Text
        );
        assert_eq!((q.as_str(), all), ("x", false));

        let (mut q, mut all) = ("graph".to_string(), false);
        edit_field(&key("cmd-a"), &mut q, &mut all, None);
        edit_field(&key("backspace"), &mut q, &mut all, None);
        assert_eq!((q.as_str(), all), ("", false));
    }

    #[test]
    fn paste_copy_and_deselect() {
        let (mut q, mut all) = ("old".to_string(), false);
        edit_field(&key("cmd-a"), &mut q, &mut all, None);
        assert_eq!(
            edit_field(&key("cmd-c"), &mut q, &mut all, None),
            FieldEdit::Copy("old".into())
        );
        edit_field(&key("cmd-v"), &mut q, &mut all, Some("new\nline"));
        assert_eq!(
            q, "newline",
            "pasted over the selection, control characters dropped"
        );
        edit_field(&key("cmd-a"), &mut q, &mut all, None);
        assert_eq!(
            edit_field(&key("left"), &mut q, &mut all, None),
            FieldEdit::Selection
        );
        assert!(!all);
        // Nothing selected: Cmd-C is not a field key; an empty paste does nothing.
        assert_eq!(
            edit_field(&key("cmd-c"), &mut q, &mut all, None),
            FieldEdit::None
        );
        assert_eq!(
            edit_field(&key("cmd-v"), &mut q, &mut all, None),
            FieldEdit::None
        );
    }

    #[test]
    fn line_and_word_deletes() {
        let (mut q, mut all) = ("show context pane".to_string(), false);
        let word = if cfg!(target_os = "macos") {
            "alt-backspace"
        } else {
            "ctrl-backspace"
        };
        edit_field(&key(word), &mut q, &mut all, None);
        assert_eq!(q, "show context ");
        edit_field(&key(word), &mut q, &mut all, None);
        assert_eq!(q, "show ");
        if cfg!(target_os = "macos") {
            edit_field(&key("cmd-backspace"), &mut q, &mut all, None);
        } else {
            // Ctrl-Backspace is word delete here; clear by selecting all first.
            edit_field(&key("ctrl-a"), &mut q, &mut all, None);
            edit_field(&key("backspace"), &mut q, &mut all, None);
        }
        assert_eq!(q, "");
        // Cmd-A on an empty query selects nothing.
        edit_field(&key("cmd-a"), &mut q, &mut all, None);
        assert!(!all);
    }

    /// Every text field gets the same keys (Cmd-A, copy, paste, word and line delete) only if
    /// no other code reads typed text itself: a hand-written field always misses some. So
    /// nothing outside this file may read `Keystroke::key_char`; a field uses `edit_field`, or
    /// `field_key` when it must apply the edits itself. (Setting `key_char`, as tests do to
    /// fabricate a keystroke, is fine.)
    #[test]
    fn only_this_file_reads_typed_text() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut dirs = vec![root.join("app/src")];
        for entry in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
            dirs.push(entry.path().join("src"));
        }
        let mut readers = Vec::new();
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("field.rs") {
                    continue;
                }
                #[allow(clippy::disallowed_methods)] // Chorong's own sources, read by a test
                let text = std::fs::read_to_string(&path).unwrap();
                for (n, line) in text.lines().enumerate() {
                    let mut rest = line;
                    while let Some(at) = rest.find(".key_char") {
                        let after = rest[at + ".key_char".len()..].trim_start();
                        let assigns = after.starts_with('=') && !after.starts_with("==");
                        if !assigns {
                            readers.push(format!("{}:{}", path.display(), n + 1));
                        }
                        rest = &rest[at + 1..];
                    }
                }
            }
        }
        assert!(
            readers.is_empty(),
            "typed text read outside field.rs (use cho_layout_ui::field::edit_field or field_key): {readers:?}"
        );
    }
}
