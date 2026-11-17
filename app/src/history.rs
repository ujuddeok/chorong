//! Back/forward history of (file, caret byte).

use std::path::PathBuf;

const MAX_ENTRIES: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    /// Caret byte offset, if any.
    pub caret: Option<usize>,
}

#[derive(Debug, Default, Clone)]
pub struct History {
    entries: Vec<Entry>,
    /// Index of the current entry; meaningless while `entries` is empty.
    cursor: usize,
}

impl History {
    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    /// Remembers where the user is now, so that coming back restores the exact caret.
    pub fn set_current_caret(&mut self, caret: Option<usize>) {
        if let Some(e) = self.entries.get_mut(self.cursor) {
            e.caret = caret;
        }
    }

    /// Records a new location and drops the forward branch. Re-recording the current
    /// location only updates it.
    pub fn push(&mut self, entry: Entry) {
        if self.entries.is_empty() {
            self.entries.push(entry);
            self.cursor = 0;
            return;
        }
        if self.entries[self.cursor] == entry {
            return;
        }
        self.entries.truncate(self.cursor + 1);
        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len() - 1;
    }

    pub fn can_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_forward(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    pub fn back(&mut self) -> Option<Entry> {
        if !self.can_back() {
            return None;
        }
        self.cursor -= 1;
        self.current().cloned()
    }

    pub fn forward(&mut self) -> Option<Entry> {
        if !self.can_forward() {
            return None;
        }
        self.cursor += 1;
        self.current().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(p: &str, l: usize) -> Entry {
        Entry {
            path: p.into(),
            caret: Some(l),
        }
    }

    #[test]
    fn back_and_forward_walk_the_entries() {
        let mut h = History::default();
        assert!(h.back().is_none());
        h.push(e("a", 1));
        h.push(e("b", 2));
        h.push(e("b", 9));
        assert_eq!(h.back(), Some(e("b", 2)));
        assert_eq!(h.back(), Some(e("a", 1)));
        assert!(h.back().is_none());
        assert_eq!(h.forward(), Some(e("b", 2)));
        assert_eq!(h.forward(), Some(e("b", 9)));
        assert!(h.forward().is_none());
    }

    #[test]
    fn pushing_after_back_drops_the_forward_branch() {
        let mut h = History::default();
        h.push(e("a", 1));
        h.push(e("b", 2));
        h.push(e("c", 3));
        h.back();
        h.back();
        h.push(e("d", 4));
        assert!(!h.can_forward());
        assert_eq!(h.back(), Some(e("a", 1)));
        assert_eq!(h.forward(), Some(e("d", 4)));
    }

    #[test]
    fn back_restores_the_exact_caret_the_user_left_on() {
        let mut h = History::default();
        h.push(e("a", 1));
        h.set_current_caret(Some(57)); // the user moved within a, then navigates
        h.push(e("b", 0));
        assert_eq!(h.back(), Some(e("a", 57)));
    }

    #[test]
    fn duplicate_push_is_ignored() {
        let mut h = History::default();
        h.push(e("a", 1));
        h.push(e("a", 1));
        assert!(!h.can_back());
    }
}
