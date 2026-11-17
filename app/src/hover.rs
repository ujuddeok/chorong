//! Hover dwell: the context pane follows the identifier under the pointer once the pointer
//! rests on it. It keeps that identifier while the pointer travels from the source into the
//! pane (so hovered references can be clicked), and goes back to the caret only when the
//! pointer has left both.
//!
//! Pure state; the view owns the timer and the background query. Every change that could
//! invalidate a pending dwell bumps `generation`, so a timer that fires late is ignored. The
//! dwell is the `context.hover_delay_ms` setting.

use crate::generation::Generation;
use std::ops::Range;

#[derive(Debug, Default)]
pub struct Hover {
    /// The identifier (file byte range) the pointer is on and a dwell timer is running for.
    pending: Option<Range<usize>>,
    /// The identifier the pane follows. `None` means the caret.
    active: Option<Range<usize>>,
    generation: Generation,
}

impl Hover {
    pub fn active(&self) -> Option<&Range<usize>> {
        self.active.as_ref()
    }

    /// The pointer moved to `ident` (`None` over whitespace, comments, the gutter). Returns
    /// the generation to start a dwell timer for, or `None` when nothing needs to start.
    pub fn moved(&mut self, ident: Option<Range<usize>>) -> Option<Generation> {
        if ident == self.pending {
            return None;
        }
        self.generation.next();
        self.pending = ident.clone();
        // Already following this identifier: no timer needed.
        match ident {
            Some(i) if self.active.as_ref() != Some(&i) => Some(self.generation),
            _ => None,
        }
    }

    /// A dwell timer fired. True when the pane should now follow the pending identifier.
    pub fn dwelled(&mut self, generation: Generation) -> bool {
        if !self.generation.is(generation) || self.pending.is_none() || self.active == self.pending
        {
            return false;
        }
        self.active = self.pending.clone();
        true
    }

    /// The pointer left the source for the pane: a dwell still running for an identifier
    /// crossed on the way is dropped, and the identifier the pane follows stays.
    pub fn cancel_pending(&mut self) {
        self.generation.next();
        self.pending = None;
    }

    /// The pointer left both the source and the pane (or the document changed). True when the pane was
    /// following a hover and must return to the caret.
    pub fn left(&mut self) -> bool {
        self.generation.next();
        self.pending = None;
        self.active.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_only_after_the_dwell_and_only_the_latest_identifier() {
        let mut h = Hover::default();
        let g1 = h.moved(Some(1..4)).unwrap();
        let g2 = h.moved(Some(10..13)).unwrap();
        assert!(!h.dwelled(g1), "a stale timer is ignored");
        assert!(h.active().is_none());
        assert!(h.dwelled(g2));
        assert_eq!(h.active(), Some(&(10..13)));
    }

    #[test]
    fn moving_within_the_same_identifier_starts_nothing() {
        let mut h = Hover::default();
        let g = h.moved(Some(1..4)).unwrap();
        assert!(h.moved(Some(1..4)).is_none());
        assert!(h.dwelled(g));
        assert!(h.moved(Some(1..4)).is_none());
    }

    #[test]
    fn leaving_the_identifier_before_the_dwell_cancels_it() {
        let mut h = Hover::default();
        let g = h.moved(Some(1..4)).unwrap();
        assert!(h.moved(None).is_none());
        assert!(!h.dwelled(g));
        assert!(h.active().is_none());
    }

    #[test]
    fn travelling_into_the_pane_keeps_the_hover_and_drops_identifiers_crossed_on_the_way() {
        let mut h = Hover::default();
        let g = h.moved(Some(1..4)).unwrap();
        assert!(h.dwelled(g));
        // On the way to the pane the pointer crosses another identifier.
        let crossed = h.moved(Some(20..25)).unwrap();
        h.cancel_pending();
        assert!(
            !h.dwelled(crossed),
            "the crossed identifier never takes over"
        );
        assert_eq!(h.active(), Some(&(1..4)));
        // Back into the source on the same identifier: no new timer, still followed.
        assert!(h.moved(Some(1..4)).is_none());
        assert_eq!(h.active(), Some(&(1..4)));
    }

    #[test]
    fn whitespace_keeps_the_last_hover_and_leaving_the_source_restores_the_caret() {
        let mut h = Hover::default();
        let g = h.moved(Some(1..4)).unwrap();
        assert!(h.dwelled(g));
        h.moved(None);
        assert!(h.active().is_some());
        assert!(h.left());
        assert!(h.active().is_none());
        assert!(!h.left(), "nothing to restore");
    }
}
