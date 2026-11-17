//! Symbol highlight groups (after Emacs hi-lock).
//!
//! Double-clicking an identifier adds a group: every identifier token with the same
//! spelling inside the innermost enclosing declaration, or, when an index answers, every
//! occurrence of the same SCIP symbol in the file. A name match is a highlight, not a claim
//! that the occurrences are the same symbol, and the mode is recorded so the UI can say
//! which one it is. Double-clicking a member of a group removes it.

use cho_syntax::Symbol;
use std::ops::Range;

/// How a group's occurrences were chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Same spelling inside a declaration (or the file) of the given kind.
    NameInScope {
        scope: Range<usize>,
        scope_kind: &'static str,
    },
    /// Same SCIP symbol, from the index.
    Symbol(String),
    /// A tour step's highlighted chunks, in reading order (the step's number). They are
    /// tinted by the step, so they draw no markers of their own; `n` / `N` move through them
    /// like any group.
    Step(String),
    /// The find bar's matches (Cmd-F), for this query. Only one find group is kept.
    Find(String),
}

impl Mode {
    /// Shown in the status bar.
    pub fn label(&self) -> String {
        match self {
            Mode::NameInScope { scope_kind, .. } => format!("same name in {scope_kind}"),
            Mode::Symbol(_) => "same symbol".into(),
            Mode::Step(_) => "tour step".into(),
            Mode::Find(_) => "find".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// Palette slot: an index into `theme.players` (taken modulo its length).
    pub slot: usize,
    pub name: String,
    pub mode: Mode,
    /// Sorted, disjoint byte ranges.
    pub occurrences: Vec<Range<usize>>,
    /// The occurrence that is the declaration, only where that is known: the SCIP definition
    /// role, or the single unambiguous syntactic binding (`let`, `const`, parameter) among a
    /// name group's occurrences. Empty when unknown or ambiguous (for instance, shadowing).
    pub declarations: Vec<Range<usize>>,
}

#[derive(Debug, Default, Clone)]
pub struct Marks {
    groups: Vec<Group>,
    /// Changes with every edit of `groups` and is never the same for two different group
    /// lists (unique across all `Marks`), so a view can cache what it drew from them.
    revision: u64,
}

fn next_revision() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Which occurrence to move to from `caret`: the one after (or before) the occurrence the
/// caret is on, wrapping; or, when the caret is between occurrences, the nearest in that
/// direction. `None` when there are no occurrences.
pub fn step_occurrence(occ: &[Range<usize>], caret: usize, forward: bool) -> Option<usize> {
    if occ.is_empty() {
        return None;
    }
    let n = occ.len();
    if let Some(on) = occurrence_at(occ, caret) {
        return Some(if forward {
            (on + 1) % n
        } else {
            (on + n - 1) % n
        });
    }
    if forward {
        Some(occ.iter().position(|r| r.start > caret).unwrap_or(0))
    } else {
        Some(occ.iter().rposition(|r| r.start < caret).unwrap_or(n - 1))
    }
}

/// The index of the occurrence the caret is on (touching either end counts), if any.
pub fn occurrence_at(occ: &[Range<usize>], caret: usize) -> Option<usize> {
    occ.iter().rposition(|r| r.start <= caret && caret <= r.end)
}

/// What a name group is computed from: the file's identifier tokens, its plain binding
/// sites, and its text.
pub struct Source<'a> {
    pub identifiers: &'a [Range<usize>],
    pub bindings: &'a [Range<usize>],
    pub text: &'a str,
}

/// The innermost declaration containing `byte`: its range and kind, or `None` for top level.
pub fn enclosing(symbols: &[Symbol], byte: usize) -> Option<&Symbol> {
    symbols
        .iter()
        .filter(|s| s.range.start <= byte && byte < s.range.end)
        .min_by_key(|s| s.range.len())
}

/// The identifier token covering `byte`, or one that ends exactly at it (a caret at the
/// end of a word is still on that word). `identifiers` is sorted and disjoint.
pub fn identifier_at(identifiers: &[Range<usize>], byte: usize) -> Option<Range<usize>> {
    let after = identifiers.partition_point(|r| r.start <= byte);
    let prev = identifiers.get(after.checked_sub(1)?)?;
    (byte <= prev.end).then(|| prev.clone())
}

/// Identifier tokens inside `scope` spelled `name`.
pub fn same_name(
    identifiers: &[Range<usize>],
    text: &str,
    name: &str,
    scope: &Range<usize>,
) -> Vec<Range<usize>> {
    let from = identifiers.partition_point(|r| r.start < scope.start);
    identifiers[from..]
        .iter()
        .take_while(|r| r.end <= scope.end)
        .filter(|r| text.get((*r).clone()) == Some(name))
        .cloned()
        .collect()
}

/// `ranges` sorted, with overlapping ones merged and empty ones dropped (an empty range would
/// mark nothing and breaks the binary searches' ordering).
fn sorted_disjoint(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.retain(|r| r.start < r.end);
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start < last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

impl Marks {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn clear(&mut self) {
        self.groups.clear();
        self.revision = next_revision();
    }

    /// Removes every group but a tour step's chunks, which stay as long as their step is
    /// open: Clear Highlights, Clear all and a double Esc remove only the reader's own.
    pub fn clear_own(&mut self) {
        self.groups.retain(|g| matches!(g.mode, Mode::Step(_)));
        self.revision = next_revision();
    }

    /// Whether there is a group other than a tour step's chunks.
    pub fn has_own(&self) -> bool {
        self.groups.iter().any(|g| !matches!(g.mode, Mode::Step(_)))
    }

    /// A number that changes whenever the groups do (see `revision`).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The groups, oldest first.
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Removes the group at `index` (oldest first). Returns whether there was one.
    pub fn remove(&mut self, index: usize) -> bool {
        self.revision = next_revision();
        (index < self.groups.len())
            .then(|| self.groups.remove(index))
            .is_some()
    }

    /// The occurrences of the newest group, sorted.
    pub fn newest_occurrences(&self) -> &[Range<usize>] {
        self.groups.last().map_or(&[], |g| &g.occurrences)
    }

    /// What `n` / `N` step through from `caret`: the newest group with an occurrence holding
    /// the caret (your highlight inside a tour chunk is yours; elsewhere in the chunk, the
    /// chunks), else the newest group.
    pub fn occurrences_at(&self, caret: usize) -> &[Range<usize>] {
        self.groups
            .iter()
            .rev()
            .find(|g| {
                let i = g.occurrences.partition_point(|r| r.end <= caret);
                g.occurrences.get(i).is_some_and(|r| r.start <= caret)
            })
            .map_or_else(|| self.newest_occurrences(), |g| &g.occurrences)
    }

    /// Removes the newest group that is not a tour step's chunks. Returns whether there was
    /// one.
    pub fn pop_newest(&mut self) -> bool {
        self.revision = next_revision();
        match self
            .groups
            .iter()
            .rposition(|g| !matches!(g.mode, Mode::Step(_)))
        {
            Some(i) => {
                self.groups.remove(i);
                true
            }
            None => false,
        }
    }

    /// The mode of the most recently added group, for the status bar.
    pub fn last_mode(&self) -> Option<&Mode> {
        self.groups.last().map(|g| &g.mode)
    }

    /// Removes every group that has `ident` as a member. Returns whether any did.
    pub fn remove_containing(&mut self, ident: &Range<usize>) -> bool {
        self.revision = next_revision();
        let before = self.groups.len();
        // A tour step's chunks span whole declarations: `h` inside one adds a group of its
        // own rather than removing the chunks.
        self.groups.retain(|g| {
            matches!(g.mode, Mode::Step(_))
                || !g
                    .occurrences
                    .iter()
                    .any(|r| r.start <= ident.start && ident.end <= r.end)
        });
        self.groups.len() != before
    }

    /// The lowest palette slot no group uses, so colours stay distinct until the palette runs out.
    fn free_slot(&self, palette: usize) -> usize {
        let palette = palette.max(1);
        (0..palette)
            .find(|s| !self.groups.iter().any(|g| g.slot % palette == *s))
            .unwrap_or(self.groups.len() % palette)
    }

    /// Adds a group by name inside `scope`. `palette` is the number of theme colours.
    pub fn add_name(
        &mut self,
        src: &Source<'_>,
        at: &Range<usize>,
        scope: Range<usize>,
        scope_kind: &'static str,
        palette: usize,
    ) {
        let (identifiers, text, bindings) = (src.identifiers, src.text, src.bindings);
        let Some(name) = text.get(at.clone()) else {
            return;
        };
        let occurrences = same_name(identifiers, text, name, &scope);
        let slot = self.free_slot(palette);
        // A declaration is claimed only when exactly one occurrence is a plain binding site.
        let mut sites = occurrences.iter().filter(|o| bindings.contains(o));
        let declarations = match (sites.next(), sites.next()) {
            (Some(one), None) => vec![one.clone()],
            _ => Vec::new(),
        };
        self.revision = next_revision();
        self.groups.push(Group {
            slot,
            name: name.to_string(),
            mode: Mode::NameInScope { scope, scope_kind },
            occurrences,
            declarations,
        });
    }

    /// Adds a group of exact occurrences of one SCIP symbol. An index is untrusted input:
    /// occurrences are sorted and any that overlap (which the checked-in indexes never have)
    /// are merged into one range, so the group keeps its sorted, disjoint invariant and the
    /// lookups below stay correct.
    pub fn add_symbol(
        &mut self,
        symbol: String,
        name: String,
        occurrences: Vec<Range<usize>>,
        definitions: Vec<Range<usize>>,
        palette: usize,
    ) {
        let occurrences = sorted_disjoint(occurrences);
        let slot = self.free_slot(palette);
        self.revision = next_revision();
        self.groups.push(Group {
            slot,
            name,
            mode: Mode::Symbol(symbol),
            occurrences,
            declarations: definitions,
        });
    }

    /// Adds (or replaces) the group of a tour step's highlighted chunks, named
    /// `Step 3 highlights`. Only one step group is kept.
    pub fn set_step(&mut self, number: &str, chunks: Vec<Range<usize>>, palette: usize) {
        self.groups.retain(|g| !matches!(g.mode, Mode::Step(_)));
        if !chunks.is_empty() {
            let slot = self.free_slot(palette);
            self.groups.push(Group {
                slot,
                name: format!("Step {number} highlights"),
                mode: Mode::Step(number.to_string()),
                occurrences: sorted_disjoint(chunks),
                declarations: Vec::new(),
            });
        }
        self.revision = next_revision();
    }

    /// Adds, replaces or (with no matches) removes the find bar's group, named after the
    /// query. A replaced group keeps its colour, so typing does not make it flicker.
    pub fn set_find(&mut self, query: &str, matches: Vec<Range<usize>>, palette: usize) {
        let old = self
            .groups
            .iter()
            .position(|g| matches!(g.mode, Mode::Find(_)));
        let slot = old.map(|i| self.groups.remove(i).slot);
        if !matches.is_empty() {
            let slot = slot.unwrap_or_else(|| self.free_slot(palette));
            self.groups.push(Group {
                slot,
                name: format!("Find \u{201c}{query}\u{201d}"),
                mode: Mode::Find(query.to_string()),
                occurrences: sorted_disjoint(matches),
                declarations: Vec::new(),
            });
        }
        self.revision = next_revision();
    }

    /// Whether the find bar's group is there.
    pub fn has_find(&self) -> bool {
        self.groups.iter().any(|g| matches!(g.mode, Mode::Find(_)))
    }

    /// The groups with an occurrence overlapping `range` (a line, or a fold's hidden lines), in
    /// group order, as `(slot, holds_the_declaration)`: what the gutter marks.
    pub fn slots_in(&self, range: &Range<usize>) -> Vec<(usize, bool)> {
        let overlaps = |r: &Range<usize>| r.start < range.end && r.end > range.start;
        self.groups
            .iter()
            .filter_map(|g| {
                let from = g.occurrences.partition_point(|r| r.end <= range.start);
                g.occurrences
                    .get(from)
                    .filter(|r| overlaps(r))
                    .map(|_| (g.slot, g.declarations.iter().any(overlaps)))
            })
            .collect()
    }

    /// Group occurrences overlapping `range` (a line), as `(slot, range, is_declaration)`,
    /// in order.
    pub fn in_range(&self, range: &Range<usize>) -> Vec<(usize, Range<usize>, bool)> {
        let mut out = Vec::new();
        for g in self
            .groups
            .iter()
            .filter(|g| !matches!(g.mode, Mode::Step(_)))
        {
            let from = g.occurrences.partition_point(|r| r.end <= range.start);
            for r in &g.occurrences[from..] {
                if r.start >= range.end {
                    break;
                }
                // By overlap, as in `slots_in`: a merged occurrence keeps its declaration.
                let declaration = g
                    .declarations
                    .iter()
                    .any(|d| d.start < r.end && d.end > r.start);
                out.push((g.slot, r.clone(), declaration));
            }
        }
        out.sort_by_key(|(_, r, _)| r.start);
        out
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn the_find_group_is_replaced_keeps_its_colour_and_draws_markers() {
        let mut m = Marks::default();
        m.add_symbol("s".into(), "a".into(), vec![0..1], vec![], 4);
        m.set_find("ab", vec![10..12, 3..5], 4);
        assert_eq!(m.groups().len(), 2);
        assert_eq!(m.groups()[1].slot, 1);
        assert_eq!(m.groups()[1].name, "Find \u{201c}ab\u{201d}");
        assert_eq!(m.last_mode().unwrap().label(), "find");
        assert_eq!(m.newest_occurrences(), &[3..5, 10..12], "n and N step it");
        assert_eq!(
            m.in_range(&(0..20)),
            [(0, 0..1, false), (1, 3..5, false), (1, 10..12, false)]
        );
        // A group added since: the find group moves back to newest when its query changes.
        m.add_symbol("t".into(), "b".into(), vec![7..8], vec![], 4);
        m.set_find("abc", vec![3..6], 4);
        assert_eq!(m.groups().len(), 3);
        assert_eq!(
            (m.groups()[2].slot, m.newest_occurrences()),
            (1, &[3..6][..])
        );
        assert!(m.has_find());
        m.set_find("abcd", Vec::new(), 4);
        assert!(!m.has_find());
        assert_eq!(m.groups().len(), 2);
    }

    #[test]
    fn a_step_group_is_replaced_not_stacked_and_draws_no_markers() {
        let mut m = Marks::default();
        m.set_step("3", vec![40..50, 10..20], 4);
        assert_eq!(m.groups().len(), 1);
        assert_eq!(m.groups()[0].name, "Step 3 highlights");
        assert_eq!(m.newest_occurrences(), &[10..20, 40..50]);
        assert!(
            m.in_range(&(0..100)).is_empty(),
            "the step tints its chunks itself"
        );
        assert_eq!(step_occurrence(m.newest_occurrences(), 12, true), Some(1));
        m.set_step("4", vec![5..6], 4);
        assert_eq!(m.groups().len(), 1);
        m.set_step("4", Vec::new(), 4);
        assert!(m.is_empty());
    }

    #[test]
    fn a_step_group_outlives_the_readers_own_and_n_follows_the_caret() {
        let mut m = Marks::default();
        m.set_step("3", vec![10..100, 200..300], 4);
        // The reader's own group, on a name inside chunk 1.
        m.groups.push(Group {
            slot: 1,
            name: "own".into(),
            mode: Mode::Symbol("s".into()),
            occurrences: vec![20..25, 250..255],
            declarations: Vec::new(),
        });
        // n / N: on the reader's name, theirs; elsewhere in a chunk, the chunks; outside
        // both, the newest.
        assert_eq!(m.occurrences_at(22), &[20..25, 250..255]);
        assert_eq!(m.occurrences_at(50), &[10..100, 200..300]);
        assert_eq!(m.occurrences_at(150), &[20..25, 250..255]);
        // `h` on a name inside a chunk never removes the chunks.
        assert!(!m.remove_containing(&(30..35)));
        assert_eq!(m.groups().len(), 2);
        assert!(
            m.remove_containing(&(20..25)),
            "the reader's own toggles off"
        );
        assert_eq!(m.groups().len(), 1);
        assert!(!m.has_own());
        // Esc and Clear Highlights leave the chunks.
        assert!(!m.pop_newest());
        m.clear_own();
        assert_eq!(m.groups().len(), 1);
    }

    use cho_syntax::{Language, analyze, outline};

    const SRC: &str = "fn a() { let x = 1; x + y }\nfn b() { let x = 2; x }\n";

    fn src_of<'a>(ids: &'a [Range<usize>], text: &'a str) -> Source<'a> {
        Source {
            identifiers: ids,
            bindings: &[],
            text,
        }
    }

    fn parts() -> (Vec<Range<usize>>, Vec<Symbol>) {
        let a = analyze(Language::Rust, SRC).unwrap();
        (a.identifiers, outline(Language::Rust, SRC))
    }

    #[test]
    fn groups_are_listed_oldest_first_and_removed_by_index() {
        let mut m = Marks::default();
        m.add_symbol("s#a".into(), "a".into(), vec![4..5, 1..2], vec![], 4);
        m.add_symbol("s#b".into(), "b".into(), vec![7..8], vec![], 4);
        let names: Vec<_> = m.groups().iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(m.groups()[0].occurrences, [1..2, 4..5]);
        assert!(!m.remove(2));
        assert!(m.remove(0));
        assert_eq!(m.groups()[0].name, "b");
        assert!(m.remove(0));
        assert!(m.is_empty());
    }

    #[test]
    fn the_gutter_lists_each_group_on_a_line_once_in_group_order() {
        let mut m = Marks::default();
        m.add_symbol(
            "s#a".into(),
            "a".into(),
            vec![2..3, 5..6, 30..31],
            vec![5..6],
            4,
        );
        m.add_symbol("s#b".into(), "b".into(), vec![8..9], vec![], 4);
        assert_eq!(m.slots_in(&(0..10)), [(0, true), (1, false)]);
        assert_eq!(m.slots_in(&(0..4)), [(0, false)]);
        assert_eq!(m.slots_in(&(10..30)), []);
        assert_eq!(m.slots_in(&(29..40)), [(0, false)]);
    }

    #[test]
    fn the_occurrence_under_the_caret_includes_both_ends() {
        let occ = [2..5, 9..12];
        assert_eq!(occurrence_at(&occ, 1), None);
        assert_eq!(occurrence_at(&occ, 2), Some(0));
        assert_eq!(occurrence_at(&occ, 5), Some(0));
        assert_eq!(occurrence_at(&occ, 7), None);
        assert_eq!(occurrence_at(&occ, 12), Some(1));
        assert_eq!(occurrence_at(&[], 0), None);
    }

    #[test]
    fn innermost_declaration_is_chosen_by_range() {
        let (_, symbols) = parts();
        let x = SRC.find("x + y").unwrap();
        assert_eq!(enclosing(&symbols, x).unwrap().name, "a");
        let nested = "mod m { fn inner() { z } }";
        let syms = outline(Language::Rust, nested);
        assert_eq!(
            enclosing(&syms, nested.find('z').unwrap()).unwrap().name,
            "inner"
        );
    }

    #[test]
    fn identifier_lookup_includes_the_end_of_a_word() {
        let (ids, _) = parts();
        let x = SRC.find("let x").unwrap() + 4;
        assert_eq!(identifier_at(&ids, x), Some(x..x + 1));
        assert_eq!(
            identifier_at(&ids, x + 1),
            Some(x..x + 1),
            "caret after the word"
        );
        assert_eq!(identifier_at(&ids, x + 2), None);
        assert_eq!(identifier_at(&[], 3), None);
    }

    #[test]
    fn name_groups_are_scoped_to_the_declaration() {
        let (ids, symbols) = parts();
        let at = identifier_at(&ids, SRC.find("let x").unwrap() + 4).unwrap();
        let a = enclosing(&symbols, at.start).unwrap();
        let mut marks = Marks::default();
        marks.add_name(&src_of(&ids, SRC), &at, a.range.clone(), "function", 5);
        let g = &marks.groups[0];
        assert_eq!(g.name, "x");
        assert_eq!(g.occurrences.len(), 2, "x in a, not the x in b");
        assert_eq!(g.mode.label(), "same name in function");
    }

    #[test]
    fn groups_coexist_with_distinct_slots_and_toggle_off() {
        let (ids, symbols) = parts();
        let x = identifier_at(&ids, SRC.find("let x").unwrap() + 4).unwrap();
        let y = identifier_at(&ids, SRC.find("y }").unwrap()).unwrap();
        let scope = enclosing(&symbols, x.start).unwrap().range.clone();
        let mut marks = Marks::default();
        marks.add_name(&src_of(&ids, SRC), &x, scope.clone(), "function", 5);
        marks.add_name(&src_of(&ids, SRC), &y, scope.clone(), "function", 5);
        let slots: Vec<_> = marks.groups.iter().map(|g| g.slot).collect();
        assert_eq!(slots, [0, 1]);
        // Removing the first frees slot 0 for the next group.
        assert!(marks.remove_containing(&x));
        assert_eq!(marks.groups.len(), 1);
        marks.add_name(&src_of(&ids, SRC), &x, scope, "function", 5);
        assert_eq!(marks.groups.last().unwrap().slot, 0);
        assert!(!marks.remove_containing(&(0..1)));
    }

    #[test]
    fn the_palette_wraps_when_exhausted() {
        let ids = vec![0..1, 2..3, 4..5];
        let text = "a b c";
        let mut marks = Marks::default();
        for r in &ids {
            marks.add_name(&src_of(&ids, text), r, 0..5, "file", 2);
        }
        let slots: Vec<_> = marks.groups.iter().map(|g| g.slot).collect();
        assert_eq!(slots, [0, 1, 0]);
    }

    #[test]
    fn symbol_groups_report_their_mode_and_line_ranges() {
        let mut marks = Marks::default();
        marks.add_symbol(
            "pkg f().".into(),
            "f".into(),
            vec![10..11, 2..3, 2..3],
            vec![2..3],
            5,
        );
        assert_eq!(marks.last_mode().unwrap().label(), "same symbol");
        assert_eq!(marks.groups[0].occurrences, [2..3, 10..11]);
        assert_eq!(marks.in_range(&(0..5)), [(0, 2..3, true)]);
        assert_eq!(marks.in_range(&(5..20)), [(0, 10..11, false)]);
        assert!(marks.in_range(&(3..10)).is_empty());
    }

    #[test]
    fn name_groups_claim_a_declaration_only_when_unambiguous() {
        let a = analyze(Language::Rust, SRC).unwrap();
        let at = identifier_at(&a.identifiers, SRC.find("let x").unwrap() + 4).unwrap();
        let mut marks = Marks::default();
        marks.add_name(
            &Source {
                identifiers: &a.identifiers,
                bindings: &a.bindings,
                text: SRC,
            },
            &at,
            0..30,
            "function",
            5,
        );
        assert_eq!(marks.groups[0].declarations, std::slice::from_ref(&at));
        // Shadowing: two `let x` in one scope means no declaration is claimed.
        let src = "fn a() { let x = 1; let x = x + 1; x }";
        let a = analyze(Language::Rust, src).unwrap();
        let at = identifier_at(&a.identifiers, src.find("let x").unwrap() + 4).unwrap();
        let mut marks = Marks::default();
        marks.add_name(
            &Source {
                identifiers: &a.identifiers,
                bindings: &a.bindings,
                text: src,
            },
            &at,
            0..src.len(),
            "function",
            5,
        );
        assert!(marks.groups[0].declarations.is_empty());
    }

    /// Found by testing `slots_in` against brute force with overlapping index occurrences:
    /// `[0..3, 1..2]` broke the binary search, so line `2..3` showed no mark.
    #[test]
    fn overlapping_index_occurrences_are_merged() {
        let mut m = Marks::default();
        m.add_symbol("s".into(), "x".into(), vec![0..3, 1..2], vec![1..2], 4);
        assert_eq!(m.groups()[0].occurrences, [0..3]);
        assert_eq!(
            m.slots_in(&(2..3)),
            [(0, false)],
            "the declaration is on 1..2"
        );
        assert_eq!(m.slots_in(&(1..2)), [(0, true)]);
        assert_eq!(m.in_range(&(2..3)), [(0, 0..3, true)]);
        m.add_symbol(
            "t".into(),
            "y".into(),
            vec![5..9, 0..4, 1..6, 7..7, 12..14, 10..12],
            vec![],
            4,
        );
        assert_eq!(m.groups()[1].occurrences, [0..9, 10..12, 12..14]);
    }

    #[test]
    fn newest_group_can_be_popped() {
        let mut marks = Marks::default();
        marks.add_symbol("s1".into(), "a".into(), vec![0..1], vec![], 5);
        marks.add_symbol("s2".into(), "b".into(), vec![4..5], vec![], 5);
        assert_eq!(marks.newest_occurrences(), [4..5]);
        assert!(marks.pop_newest());
        assert_eq!(marks.newest_occurrences(), [0..1]);
        assert!(marks.pop_newest());
        assert!(!marks.pop_newest());
    }

    #[test]
    fn stepping_wraps_and_starts_from_between_occurrences() {
        let occ = [2..3, 10..11, 20..21];
        assert_eq!(step_occurrence(&occ, 0, true), Some(0));
        assert_eq!(step_occurrence(&occ, 2, true), Some(1), "on an occurrence");
        assert_eq!(step_occurrence(&occ, 20, true), Some(0), "wraps");
        assert_eq!(step_occurrence(&occ, 2, false), Some(2), "wraps backwards");
        assert_eq!(step_occurrence(&occ, 15, false), Some(1));
        assert_eq!(step_occurrence(&occ, 15, true), Some(2));
        assert_eq!(step_occurrence(&[], 15, true), None);
    }
}
