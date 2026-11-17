//! Mapping between display positions and UTF-8 byte offsets.
//!
//! A source line is shown with tabs expanded to spaces. `document::expand_tabs` gives
//! the forward map (byte in the line -> display offset); this module is its inverse, so
//! a pointer position found in the display text becomes a byte offset in the file.

/// The byte offset within a raw line for display offset `display`.
///
/// `map` is the forward map from `expand_tabs` (`raw_len + 1` entries), or `None` for a
/// line without tabs, where the two offsets are equal. A display offset inside a tab's
/// expansion maps to the tab itself; one past the end maps to `raw_len`. The result is
/// always a character boundary of the raw line.
pub fn display_to_byte(map: Option<&[usize]>, raw_len: usize, display: usize) -> usize {
    let Some(map) = map else {
        return display.min(raw_len);
    };
    // The last byte whose display offset is <= `display`, then back to the first byte
    // sharing that display offset (the start of the character).
    let after = map.partition_point(|&m| m <= display);
    let Some(last) = after.checked_sub(1) else {
        return 0;
    };
    let value = map[last];
    map.partition_point(|&m| m < value).min(raw_len)
}

/// How many times the button has been pressed in quick succession.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickKind {
    Single,
    Double,
    /// Three or more presses. Treated as a plain caret placement.
    Other,
}

impl ClickKind {
    pub fn from_count(count: usize) -> Self {
        match count {
            0 | 1 => Self::Single,
            2 => Self::Double,
            _ => Self::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::expand_tabs;

    #[test]
    fn without_tabs_offsets_are_equal_and_clamped() {
        assert_eq!(display_to_byte(None, 5, 3), 3);
        assert_eq!(display_to_byte(None, 5, 99), 5);
    }

    #[test]
    fn inverse_of_expand_tabs_at_every_character_start() {
        let raw = "a\tbc\t\té x";
        let (_, map) = expand_tabs(raw);
        for (byte, _) in raw.char_indices() {
            assert_eq!(
                display_to_byte(Some(&map), raw.len(), map[byte]),
                byte,
                "byte {byte}"
            );
        }
        assert_eq!(
            display_to_byte(Some(&map), raw.len(), map[raw.len()]),
            raw.len()
        );
    }

    #[test]
    fn inside_a_tab_lands_on_the_tab_and_past_the_end_on_the_end() {
        let raw = "a\tb";
        let (text, map) = expand_tabs(raw);
        assert_eq!(text, "a   b");
        for d in 1..4 {
            assert_eq!(display_to_byte(Some(&map), raw.len(), d), 1);
        }
        assert_eq!(display_to_byte(Some(&map), raw.len(), 4), 2);
        assert_eq!(display_to_byte(Some(&map), raw.len(), 40), raw.len());
    }

    #[test]
    fn multibyte_characters_never_split() {
        let raw = "\té\t";
        let (_, map) = expand_tabs(raw);
        for d in 0..12 {
            let b = display_to_byte(Some(&map), raw.len(), d);
            assert!(raw.is_char_boundary(b), "display {d} -> {b}");
        }
    }

    #[test]
    fn click_counts() {
        assert_eq!(ClickKind::from_count(1), ClickKind::Single);
        assert_eq!(ClickKind::from_count(2), ClickKind::Double);
        assert_eq!(ClickKind::from_count(3), ClickKind::Other);
    }
}
