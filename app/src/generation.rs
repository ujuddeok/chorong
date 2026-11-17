//! Request counters for background work whose result may arrive after a newer request.

/// A request counter for background work whose result may arrive after a newer request:
/// `next()` starts a request and returns its ticket; a result lands only if `is(ticket)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct Generation(u64);

impl Generation {
    /// Starts a new request; earlier tickets stop being current.
    pub(crate) fn next(&mut self) -> Generation {
        self.0 += 1;
        *self
    }

    /// Whether `ticket` is the latest request.
    pub(crate) fn is(self, ticket: Generation) -> bool {
        self == ticket
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_latest_ticket_is_current() {
        let mut g = Generation::default();
        let first = g.next();
        assert!(g.is(first));
        let second = g.next();
        assert!(!g.is(first), "a newer request supersedes it");
        assert!(g.is(second));
        assert_ne!(first, second);
    }
}
