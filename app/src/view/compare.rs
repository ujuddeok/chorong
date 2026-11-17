//! "Compare against…": a local Before/After comparison in the viewer. The window has at most
//! one `Comparison` (the merge base of HEAD and what the user picked); each editor pane shows
//! its file on one `Side`. After is the working-folder file; Before is the same file read from
//! the merge base's object store, syntax only (treated like "View at rev"). Switching sides
//! keeps the place by declaration, and the gutter marks what changed.
//!
//! Positions are UTF-8 byte offsets; the pure helpers here work in zero-based lines.

use super::changes::{ChangePlace, matching_symbols};
use super::compare_git as git;
use super::editor_state::Anchor;
use super::navigation::Reveal;
use super::*;
use cho_git::linediff::DEFAULT_MAX_COST;
use cho_git::{LineHunk, diff_lines};
use cho_syntax::SymbolKind;
use cho_theme::Theme;
use git::FileHunk;
use std::path::Path;

/// The comparison the window is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Comparison {
    /// The merge base of HEAD and `against`, as a full commit id.
    pub base: String,
    /// What the user picked ("main", a tag, or a typed commit).
    pub against: String,
    /// How many merge bases there were (the newest is used).
    pub merge_bases: usize,
}

/// Which version of the file a pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Side {
    /// The working-folder file.
    #[default]
    After,
    /// The file at the merge base.
    Before,
}

impl Side {
    pub fn flip(self) -> Side {
        match self {
            Side::After => Side::Before,
            Side::Before => Side::After,
        }
    }
}

// ---- Gutter marks ----

const CHANGED: u8 = 1;
const WEDGE: u8 = 2;
const HOLLOW: u8 = 4;
const WEDGE_HOLLOW: u8 = 8;

/// What the gutter draws for one side of a file: per line, whether it changed and whether a
/// wedge sits on the boundary above it (the other side has lines this side lacks). Lines (and
/// wedges) that only uncommitted work touches are marked apart from committed ones, on both
/// sides. One extra entry stands for the boundary after the last line. Computed once when a
/// document is prepared, never per frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Gutter {
    flags: Vec<u8>,
    /// The wedges, by boundary: what the other side has there.
    gaps: Vec<Gap>,
}

/// A place where the other side has lines this side lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gap {
    /// The boundary above this line (the line count for the end of the file).
    pub line: usize,
    /// How many lines the other side has here.
    pub count: usize,
    /// The first of them, a zero-based line on the other side.
    pub other: usize,
    /// Only uncommitted work made the gap.
    pub uncommitted: bool,
}

impl Gutter {
    /// Marks for `side` from the hunks (base = Before, head = After). Lines changed on this
    /// side are marked; a hunk with nothing on this side leaves a wedge at its start. A hunk
    /// uncommitted work touches is marked hollow. A `coarse` diff is one hunk, so it marks the
    /// whole range.
    pub fn from_hunks(hunks: &[FileHunk], side: Side, lines: usize) -> Gutter {
        let mut flags = vec![0u8; lines + 1];
        let mut gaps: Vec<Gap> = Vec::new();
        for fh in hunks {
            let h = &fh.hunk;
            let hollow = fh.uncommitted;
            let (r, o) = match side {
                Side::After => (&h.head, &h.base),
                Side::Before => (&h.base, &h.head),
            };
            if r.is_empty() {
                let at = r.start.min(lines);
                let f = &mut flags[at];
                if hollow {
                    if *f & WEDGE == 0 {
                        *f |= WEDGE | WEDGE_HOLLOW;
                    }
                } else {
                    *f = (*f | WEDGE) & !WEDGE_HOLLOW;
                }
                match gaps.iter_mut().find(|g| g.line == at) {
                    Some(g) => {
                        g.count += o.len();
                        g.other = g.other.min(o.start);
                        g.uncommitted &= hollow;
                    }
                    None => gaps.push(Gap {
                        line: at,
                        count: o.len(),
                        other: o.start,
                        uncommitted: hollow,
                    }),
                }
            }
            for f in &mut flags[r.start.min(lines)..r.end.min(lines)] {
                if hollow {
                    if *f & CHANGED == 0 {
                        *f |= HOLLOW;
                    }
                } else {
                    *f = (*f | CHANGED) & !HOLLOW;
                }
            }
        }
        gaps.sort_by_key(|g| g.line);
        Gutter { flags, gaps }
    }

    /// A hollow mark: only uncommitted work changed this line.
    pub fn hollow(&self, line: usize) -> bool {
        self.flags.get(line).is_some_and(|f| f & HOLLOW != 0)
    }

    /// The wedge above `line` (or at the end) is only uncommitted work.
    pub fn wedge_is_hollow(&self, line: usize) -> bool {
        self.flags.get(line).is_some_and(|f| f & WEDGE_HOLLOW != 0)
    }

    pub fn changed(&self, line: usize) -> bool {
        self.flags.get(line).is_some_and(|f| f & CHANGED != 0)
    }

    /// Committed or uncommitted work changed this line.
    pub fn any_change(&self, line: usize) -> bool {
        self.flags
            .get(line)
            .is_some_and(|f| f & (CHANGED | HOLLOW) != 0)
    }

    /// A wedge on the boundary above `line`.
    pub fn wedge_above(&self, line: usize) -> bool {
        self.flags.get(line).is_some_and(|f| f & WEDGE != 0)
    }

    /// A wedge after the last line.
    pub fn wedge_at_end(&self) -> bool {
        self.flags.last().is_some_and(|f| f & WEDGE != 0)
    }

    /// Where each stretch of uncommitted work starts, in order: the first line of each run of
    /// lines only uncommitted work changed, and each boundary where it removed lines (clamped
    /// to the last line). What the header counts and `u` / Shift-U step through.
    pub fn uncommitted_runs(&self) -> Vec<usize> {
        let lines = self.flags.len().saturating_sub(1);
        let only = |l: usize| self.hollow(l) && !self.changed(l);
        let mut out: Vec<usize> = (0..lines)
            .filter(|&l| only(l) && (l == 0 || !only(l - 1)))
            .collect();
        out.extend(
            self.gaps
                .iter()
                .filter(|g| g.uncommitted)
                .map(|g| g.line.min(lines.saturating_sub(1)))
                // A deletion right above a run is part of it.
                .filter(|l| !only(*l)),
        );
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The gap on the boundary above `line` (the line count for the end).
    pub fn gap(&self, line: usize) -> Option<&Gap> {
        self.gaps
            .binary_search_by_key(&line, |g| g.line)
            .ok()
            .map(|i| &self.gaps[i])
    }
}

/// The run start after `line` (`forward`) or before it, wrapping round; the first one when
/// there is no caret.
pub(crate) fn next_run(runs: &[usize], line: Option<usize>, forward: bool) -> Option<usize> {
    let line = match line {
        Some(l) => l,
        None => return runs.first().copied(),
    };
    if forward {
        runs.iter()
            .copied()
            .find(|r| *r > line)
            .or(runs.first().copied())
    } else {
        runs.iter()
            .rev()
            .copied()
            .find(|r| *r < line)
            .or(runs.last().copied())
    }
}

/// What a deletion marker's tooltip says, on `side`, for `gap`.
pub(crate) fn gap_text(side: Side, gap: &Gap) -> String {
    let lines = if gap.count == 1 { "line" } else { "lines" };
    let by = if gap.uncommitted {
        " by uncommitted work"
    } else {
        ""
    };
    match side {
        Side::After => format!(
            "{} {lines} removed here{by} \u{b7} click to see Before",
            gap.count
        ),
        Side::Before => format!(
            "{} {lines} added here on After{by} \u{b7} click to see After",
            gap.count
        ),
    }
}

// ---- Changed words ----

/// The changed words of a file's changed lines on both sides, from
/// [`cho_git::worddiff::paired_lines`] run over each hunk. Byte ranges are within a line.
/// Computed in the background with the diff, never per frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Words {
    /// Index 0 is Before, 1 After: line → changed byte ranges within it, in order.
    spans: [HashMap<usize, Vec<std::ops::Range<usize>>>; 2],
    /// Lines paired with a line that differs only in whitespace.
    whitespace: [HashSet<usize>; 2],
}

impl Words {
    pub fn compute(base: &[u8], head: &[u8], hunks: &[FileHunk]) -> Words {
        use cho_git::Lines;
        let (lb, lh) = (Lines::new(base), Lines::new(head));
        fn text<'a>(l: &Lines<'a>, i: usize) -> Option<&'a str> {
            std::str::from_utf8(l.line(i))
                .ok()
                .map(|t| t.trim_end_matches(['\n', '\r']))
        }
        let mut w = Words::default();
        for fh in hunks {
            let h = &fh.hunk;
            if h.base.is_empty()
                || h.head.is_empty()
                || h.base.end > lb.len()
                || h.head.end > lh.len()
            {
                continue;
            }
            let b: Option<Vec<&str>> = h.base.clone().map(|i| text(&lb, i)).collect();
            let a: Option<Vec<&str>> = h.head.clone().map(|i| text(&lh, i)).collect();
            let (Some(b), Some(a)) = (b, a) else { continue };
            for p in cho_git::worddiff::paired_lines(&b, &a) {
                let (bl, hl) = (h.base.start + p.base, h.head.start + p.head);
                if p.whitespace_only {
                    w.whitespace[0].insert(bl);
                    w.whitespace[1].insert(hl);
                }
                if !p.base_spans.is_empty() {
                    w.spans[0].insert(bl, p.base_spans);
                }
                if !p.head_spans.is_empty() {
                    w.spans[1].insert(hl, p.head_spans);
                }
            }
        }
        w
    }

    fn ix(side: Side) -> usize {
        match side {
            Side::Before => 0,
            Side::After => 1,
        }
    }

    /// The changed words of `line` on `side`, as byte ranges within the line.
    pub fn on(&self, side: Side, line: usize) -> &[std::ops::Range<usize>] {
        self.spans[Self::ix(side)]
            .get(&line)
            .map_or(&[], Vec::as_slice)
    }

    /// `line` on `side` differs from its pair only in whitespace.
    pub fn whitespace_only(&self, side: Side, line: usize) -> bool {
        self.whitespace[Self::ix(side)].contains(&line)
    }
}

// ---- Keeping the place ----

/// One declaration, for matching across the two sides: (qualified name, kind, nth with that
/// pair in the file). The file itself is matched by path before this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decl {
    pub name: String,
    pub kind: SymbolKind,
    pub ordinal: usize,
    pub first_line: usize,
    pub last_line: usize,
}

pub(crate) fn decls(doc: &Document) -> Vec<Decl> {
    let Some(lang) = doc.language else {
        return Vec::new();
    };
    let mut out: Vec<Decl> = Vec::new();
    for s in &doc.symbols {
        let name = s.qualified_name(lang);
        let ordinal = out
            .iter()
            .filter(|d| d.name == name && d.kind == s.kind)
            .count();
        out.push(Decl {
            name,
            kind: s.kind,
            ordinal,
            first_line: doc.buffer.line_of(s.range.start),
            last_line: doc.buffer.line_of(s.range.end.saturating_sub(1)),
        });
    }
    out
}

/// The innermost declaration containing `line`.
fn innermost(decls: &[Decl], line: usize) -> Option<&Decl> {
    innermost_index(decls, line).map(|i| &decls[i])
}

/// The index of the innermost declaration containing `line`. No allocation.
pub(crate) fn innermost_index(decls: &[Decl], line: usize) -> Option<usize> {
    (0..decls.len())
        .filter(|&i| decls[i].first_line <= line && line <= decls[i].last_line)
        .max_by_key(|&i| (decls[i].first_line, std::cmp::Reverse(decls[i].last_line)))
}

/// A linked split: the Before pane and the After pane showing the same file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SplitLink {
    pub before: PaneId,
    pub after: PaneId,
    /// Following each other's declaration (the toggle turns it off without ending the pair).
    pub linked: bool,
}

impl SplitLink {
    pub fn has(&self, pane: PaneId) -> bool {
        self.before == pane || self.after == pane
    }

    /// The other pane of the pair.
    pub fn other(&self, pane: PaneId) -> PaneId {
        if pane == self.before {
            self.after
        } else {
            self.before
        }
    }
}

/// The links left when `pane` is opened on another file, switched or otherwise leaves its pair.
pub(crate) fn links_without(links: &[SplitLink], pane: PaneId) -> Vec<SplitLink> {
    links.iter().copied().filter(|l| !l.has(pane)).collect()
}

/// The links whose two panes both still exist.
pub(crate) fn links_among(links: &[SplitLink], panes: &[PaneId]) -> Vec<SplitLink> {
    links
        .iter()
        .copied()
        .filter(|l| panes.contains(&l.before) && panes.contains(&l.after))
        .collect()
}

/// Whether a caret move should move the other pane: only when the innermost declaration
/// around the caret is not the one seen at the last check (`None` = never checked).
pub(crate) fn declaration_changed(last: Option<Option<usize>>, now: Option<usize>) -> bool {
    last != Some(now)
}

/// Where the caret goes on the other side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Placed {
    /// A line of the other side; no message needed.
    Line(usize),
    /// The declaration exists on this side only; the line is mapped through the line diff.
    Only {
        name: String,
        line: usize,
        /// `true` when it exists only on After (added in this change).
        added: bool,
    },
}

/// Maps a line of the `from` side to the other side through the hunks (old = Before, new =
/// After): lines after a hunk shift by the sizes before it, a line inside a hunk goes to the
/// same offset in the other side's range, clamped.
pub(crate) fn map_line(hunks: &[LineHunk], line: usize, from: Side) -> usize {
    let mut delta: isize = 0;
    for h in hunks {
        let (src, dst) = match from {
            Side::Before => (&h.base, &h.head),
            Side::After => (&h.head, &h.base),
        };
        if line < src.start {
            break;
        }
        if line < src.end {
            let off = line - src.start;
            return dst.start + off.min(dst.len().saturating_sub(1));
        }
        delta += dst.len() as isize - src.len() as isize;
    }
    (line as isize + delta).max(0) as usize
}

/// Keeps the caret's place when switching from `from_side`: the innermost declaration
/// containing the caret line is found on the other side by (name, kind, ordinal) and the
/// caret keeps its line offset inside it (clamped). With no such declaration on the other
/// side, or none around the caret, the line is mapped through the line diff.
pub(crate) fn keep_place(
    from: &[Decl],
    to: &[Decl],
    caret_line: usize,
    hunks: &[LineHunk],
    from_side: Side,
) -> Placed {
    let mapped = map_line(hunks, caret_line, from_side);
    let Some(d) = innermost(from, caret_line) else {
        return Placed::Line(mapped);
    };
    match to
        .iter()
        .find(|t| t.name == d.name && t.kind == d.kind && t.ordinal == d.ordinal)
    {
        Some(t) => Placed::Line((t.first_line + (caret_line - d.first_line)).min(t.last_line)),
        None => Placed::Only {
            name: d.name.clone(),
            line: mapped,
            added: from_side == Side::After,
        },
    }
}

// ---- Picker support ----

/// Whether `text` could be a commit id typed by hand: 4 to 40 hex digits.
pub(crate) fn looks_like_commit(text: &str) -> bool {
    (4..=40).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The picker rows for `refs`: the default branch first, then the current branch's upstream,
/// then the rest in the order given (kind, then name). A remote branch is dropped when a local
/// branch of the same name points at the same commit. Returns (label, hint, rev).
pub(crate) fn ref_rows(
    refs: &[cho_git::RefEntry],
    preferred: &cho_git::PreferredRefs,
) -> Vec<(String, String, String)> {
    use cho_git::RefKind;
    let local: HashSet<(&str, &str)> = refs
        .iter()
        .filter(|r| r.kind == RefKind::Branch)
        .map(|r| (r.short.as_str(), r.target.as_str()))
        .collect();
    let kept: Vec<&cho_git::RefEntry> = refs
        .iter()
        .filter(|r| {
            r.kind != RefKind::RemoteBranch
                || r.short
                    .split_once('/')
                    .is_none_or(|(_, name)| !local.contains(&(name, r.target.as_str())))
        })
        .collect();
    // The default branch: the local one, else a remote one of that name.
    let is_default = |r: &cho_git::RefEntry| match (&preferred.default_branch, r.kind) {
        (Some(d), RefKind::Branch) => r.short == *d,
        (Some(d), RefKind::RemoteBranch) => r.short.split_once('/').is_some_and(|(_, n)| n == d),
        _ => false,
    };
    let default = kept
        .iter()
        .position(|r| r.kind == RefKind::Branch && is_default(r))
        .or_else(|| kept.iter().position(|r| is_default(r)));
    let upstream = preferred
        .upstream
        .as_ref()
        .and_then(|u| kept.iter().position(|r| r.short == *u))
        .filter(|i| Some(*i) != default);
    let mut order: Vec<usize> = default.into_iter().chain(upstream).collect();
    let rest: Vec<usize> = (0..kept.len()).filter(|i| !order.contains(i)).collect();
    order.extend(rest);
    order
        .into_iter()
        .map(|i| {
            let r = kept[i];
            let kind = if Some(i) == default {
                "default branch"
            } else if Some(i) == upstream {
                "upstream"
            } else {
                match r.kind {
                    RefKind::Branch => "branch",
                    RefKind::RemoteBranch => "remote branch",
                    RefKind::Tag => "tag",
                }
            };
            (
                r.short.clone(),
                format!("{kind} \u{b7} {}", cho_git::short(&r.target)),
                r.short.clone(),
            )
        })
        .collect()
}

/// A byte offset in the line `range` of `text` that keeps column `col` (bytes from the line
/// start), clamped to the line's length and rounded down to a character boundary.
pub(crate) fn byte_in_line(text: &str, range: std::ops::Range<usize>, col: usize) -> usize {
    let mut at = (range.start + col).min(range.end);
    while at > range.start && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The notice after a comparison is set.
pub(crate) fn comparing_notice(against: &str, base: &str, merge_bases: usize) -> String {
    // No merge base was computed: Before was given (a link's `base`, or `--base`).
    if merge_bases == 0 {
        return format!(
            "Comparing with {against}: Before is {}, as given",
            cho_git::short(base)
        );
    }
    let mut text = format!(
        "Comparing with {against}: Before is {}, the merge base",
        cho_git::short(base)
    );
    if merge_bases > 1 {
        text.push_str(&format!(". {merge_bases} merge bases; using the newest"));
    }
    text
}

/// The other side of a pane's file, built in the background when a comparison starts or a file
/// opens while comparing, so switching only swaps documents. Holds the other side's parsed
/// document (highlighting and outline included) and the line diff between the two sides.
#[derive(Clone)]
pub(crate) struct Prepared {
    /// Which side `doc` is.
    pub side: Side,
    /// The merge base this was built for.
    pub base: String,
    /// The `EditorState::epoch` of the document it complements.
    pub for_epoch: u64,
    pub built: Built,
}

/// One side of a file as built for a comparison.
#[derive(Clone)]
pub(crate) struct Built {
    /// `None` when the file is not at the merge base (a new file): there is no Before.
    pub doc: Option<Arc<Document>>,
    pub facts: SideFacts,
}

/// What goes with a side's document: its declarations, the diff and whether an index answers
/// for it.
#[derive(Clone)]
pub(crate) struct SideFacts {
    /// The declarations of the document, for keeping the place.
    pub decls: Arc<Vec<Decl>>,
    /// Base (Before) to working-folder (After) hunks; the same diff serves both sides'
    /// gutters, and After's tells committed from uncommitted.
    pub diff: Arc<Diff>,
    /// An index built at the document's commit answers for it.
    pub index_ok: bool,
}

/// The hunks between a file at the merge base and in the working folder.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Diff {
    pub hunks: Vec<FileHunk>,
    /// `hunks` as plain line hunks, for mapping lines.
    pub lines: Arc<Vec<LineHunk>>,
    /// The diff hit its cost limit: `hunks` is one block.
    pub coarse: bool,
    /// The changed words within paired lines.
    pub words: Words,
}

impl Diff {
    pub fn new(hunks: Vec<FileHunk>, coarse: bool, words: Words) -> Diff {
        let lines = Arc::new(hunks.iter().map(|h| h.hunk.clone()).collect());
        Diff {
            hunks,
            lines,
            coarse,
            words,
        }
    }
}

/// Reads the merge-base and working-folder versions of `path`, diffs them and parses the
/// `target` side. Runs on the background executor.
fn build_other(
    root: &Path,
    cmp_base: &str,
    path: &Path,
    target: Side,
    after_display: String,
    theme: &Theme,
    json_folds: bool,
) -> Result<Built, String> {
    let base = git::file_diff(root, cmp_base, path)?;
    // The working file is read here but only required by the After side: Before comes from
    // git, so it still opens when the working file is deleted, unreadable or too large (it then
    // has no word-level marks).
    let after =
        cho_core::fs::read_source_in(root, path).map_err(|e| format!("{}: {e}", path.display()));
    let started = Instant::now();
    let words = match (&base.bytes, &after, base.coarse) {
        (Some(b), Ok(after), false) => Words::compute(b, after, &base.hunks),
        _ => Words::default(),
    };
    if crate::devlog::on() {
        eprintln!("WORDS us={}", started.elapsed().as_micros());
    }
    let diff = Diff::new(base.hunks.clone(), base.coarse, words);
    let (doc, index_ok) = match target {
        Side::Before => match (base.bytes.clone(), &base.path) {
            (Some(bytes), Some(bpath)) => {
                let display = base
                    .top
                    .join(bpath)
                    .strip_prefix(root)
                    .map_or(bpath.clone(), |p| p.display().to_string());
                let d = Document::from_bytes(
                    path.to_path_buf(),
                    display,
                    bytes,
                    Some(cho_git::short(cmp_base)),
                    theme,
                    json_folds,
                )
                .map_err(|e| e.to_string())?;
                (Some(d), mismatch::rev_index_matches(&base.top, cmp_base))
            }
            _ => (None, false),
        },
        Side::After => {
            let d = Document::from_bytes(
                path.to_path_buf(),
                after_display,
                after?,
                None,
                theme,
                json_folds,
            )
            .map_err(|e| e.to_string())?;
            (Some(d), false)
        }
    };
    let decls = Arc::new(doc.as_ref().map(decls).unwrap_or_default());
    Ok(Built {
        doc: doc.map(Arc::new),
        facts: SideFacts {
            decls,
            diff: Arc::new(diff),
            index_ok,
        },
    })
}

/// Generations of the Compare picker's background git work: opening the picker starts a
/// refs lookup, each query edit a typed-commit lookup. An answer lands only while its
/// generation is still the newest of its kind.
#[derive(Debug, Default)]
pub(super) struct PickerJobs {
    refs: Generation,
    commit: Generation,
    /// Enter was pressed while the refs lookup of this generation ran: it applies on landing.
    confirm: Option<Generation>,
}

impl PickerJobs {
    fn next_refs(&mut self) -> Generation {
        self.refs.next()
    }

    fn refs_current(&self, generation: Generation) -> bool {
        self.refs.is(generation)
    }

    fn next_commit(&mut self) -> Generation {
        self.commit.next()
    }

    fn commit_current(&self, generation: Generation) -> bool {
        self.commit.is(generation)
    }

    /// Holds a confirm until the current refs lookup lands.
    fn defer_confirm(&mut self) {
        self.confirm = Some(self.refs);
    }

    /// Whether a confirm was held for the refs lookup `generation` (and forgets it).
    fn take_confirm(&mut self, generation: Generation) -> bool {
        self.confirm.take() == Some(generation)
    }
}

/// The Compare picker is still listing branches and tags.
fn refs_loading(p: &PickerState) -> bool {
    p.kind == PickerKind::Compare && p.hints.iter().any(|h| h == LOADING_HINT)
}

/// The ref (or typed commit) the selected row stands for.
fn selected_ref(p: &PickerState) -> Option<(String, String)> {
    let m = p.matches.get(p.selected)?;
    match p.picks.get(m.index)? {
        Pick::Ref { label, rev } => Some((label.clone(), rev.clone())),
        _ => None,
    }
}

const COMMIT_HINT: &str = "commit";
const LOADING_HINT: &str = "loading";

/// The row the Compare picker shows until its branches and tags are listed.
fn loading_row() -> (String, String, Pick) {
    (
        "Listing branches and tags\u{2026}".into(),
        LOADING_HINT.into(),
        Pick::Unavailable("Still listing branches and tags".into()),
    )
}

/// Replaces the loading row with the ref rows, keeping a typed-commit row last.
fn fill_ref_rows(p: &mut PickerState, rows: Vec<(String, String, String)>) {
    let commit = take_commit_row(p);
    if p.hints.last().is_some_and(|h| h == LOADING_HINT) {
        p.labels.pop();
        p.hints.pop();
        p.picks.pop();
    }
    for (label, hint, rev) in rows {
        p.labels.push(label.clone());
        p.hints.push(hint);
        p.picks.push(Pick::Ref { label, rev });
    }
    if let Some((label, rev)) = commit {
        set_commit_row(p, Some((label, rev)));
    }
}

/// Removes the typed-commit row (always last) and returns its label and revision.
fn take_commit_row(p: &mut PickerState) -> Option<(String, String)> {
    if !p.hints.last().is_some_and(|h| h == COMMIT_HINT) {
        return None;
    }
    p.labels.pop();
    p.hints.pop();
    match p.picks.pop() {
        Some(Pick::Ref { label, rev }) => Some((label, rev)),
        _ => None,
    }
}

/// Sets (or, with `None`, clears) the typed-commit row at the end of the Compare picker.
fn set_commit_row(p: &mut PickerState, row: Option<(String, String)>) {
    take_commit_row(p);
    if let Some((label, rev)) = row {
        p.labels.push(label.clone());
        p.hints.push(COMMIT_HINT.into());
        p.picks.push(Pick::Ref { label, rev });
    }
}

/// Lands a typed-commit lookup for `query` (`full`: the commit it resolved to): only on a
/// Compare picker whose query is still `query`. Returns whether a row was added.
fn land_commit_row(p: &mut PickerState, query: &str, full: Option<String>) -> bool {
    if p.kind != PickerKind::Compare || p.query.trim() != query {
        return false;
    }
    let Some(full) = full else { return false };
    set_commit_row(p, Some((query.to_string(), full)));
    true
}

/// Everything `land` needs to put a prepared or freshly built document on screen.
struct Landing {
    doc: Arc<Document>,
    facts: SideFacts,
    /// The hunks the caret is mapped through (differ from `diff` when Before is reloaded at
    /// a new base: old Before to new Before).
    place: Arc<Vec<LineHunk>>,
    /// Before was reloaded at a new base (same side): the note wording differs.
    reload: bool,
}

impl Chorong {
    // ---- Choosing what to compare with ----

    /// Opens the Compare picker at once and lists the branches and tags off the UI thread
    /// (`git for-each-ref` and the default-branch lookup can take a while in a big repository).
    pub(super) fn open_compare_picker(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.notify_text("Open a folder in a git repository first", cx);
            return;
        };
        let (label, hint, pick) = loading_row();
        self.open_overlay_picker(PickerKind::Compare, vec![label], vec![hint], vec![pick]);
        let generation = self.compare_jobs.next_refs();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    cho_git::list_refs(&root).map(|refs| {
                        let preferred = cho_git::preferred_refs(&root);
                        ref_rows(&refs, &preferred)
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                if !this.compare_jobs.refs_current(generation) {
                    return;
                }
                let Some(p) = this
                    .picker
                    .as_mut()
                    .filter(|p| p.kind == PickerKind::Compare)
                else {
                    return;
                };
                match found {
                    Ok(rows) => {
                        fill_ref_rows(p, rows);
                        p.matches = this.ranker.rank(&p.labels, &p.query, MAX_RESULTS);
                        p.selected = p.selected.min(p.matches.len().saturating_sub(1));
                        // Enter pressed while listing: confirm the selected row now. With no
                        // row, the picker stays open and shows "No matches".
                        if this.compare_jobs.take_confirm(generation)
                            && let Some((label, rev)) = selected_ref(p)
                        {
                            this.picker = None;
                            this.choose_comparison(label, rev, cx);
                        }
                    }
                    Err(e) => {
                        // As before the lookup moved off the UI thread: no picker, a notice.
                        this.picker = None;
                        this.notify_text(e, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Keeps the typed-commit row of the Compare picker in step with the query: the row goes
    /// at once, and a query that looks like a commit is resolved off the UI thread after a
    /// short pause in typing. An answer for an older query is dropped.
    pub(super) fn compare_picker_query(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let Some(p) = &mut self.picker else { return };
        if p.kind != PickerKind::Compare {
            return;
        }
        set_commit_row(p, None);
        let generation = self.compare_jobs.next_commit();
        let q = p.query.trim().to_string();
        let Some(root) = root.filter(|_| looks_like_commit(&q)) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let current = this
                .update(cx, |this, _| this.compare_jobs.commit_current(generation))
                .unwrap_or(false);
            if !current {
                return;
            }
            let query = q.clone();
            let full = cx
                .background_spawn(async move { cho_git::rev_parse(&root, &query) })
                .await;
            this.update(cx, |this, cx| {
                if !this.compare_jobs.commit_current(generation) {
                    return;
                }
                let Some(p) = this.picker.as_mut() else {
                    return;
                };
                if land_commit_row(p, &q, full) {
                    p.matches = this.ranker.rank(&p.labels, &p.query, MAX_RESULTS);
                    p.selected = p.selected.min(p.matches.len().saturating_sub(1));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Enter before a typed commit's lookup has landed: resolves it now (one `git rev-parse`,
    /// on an explicit confirm only), so Enter still picks the typed commit rather than a ref
    /// that happens to match the query.
    pub(super) fn compare_confirm_typed(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(p) = &mut self.picker else { return };
        if p.kind != PickerKind::Compare || p.hints.last().is_some_and(|h| h == COMMIT_HINT) {
            return;
        }
        let q = p.query.trim().to_string();
        if !looks_like_commit(&q) {
            return;
        }
        // The pending lookup is answered here; its own answer is dropped.
        self.compare_jobs.next_commit();
        if land_commit_row(p, &q, cho_git::rev_parse(&root, &q)) {
            p.matches = self.ranker.rank(&p.labels, &p.query, MAX_RESULTS);
            p.selected = p.selected.min(p.matches.len().saturating_sub(1));
        }
    }

    /// Confirming the Compare picker while its branches and tags are still being listed:
    /// the confirm is held and applied when the list lands, and the picker stays open.
    pub(super) fn compare_defer_confirm(&mut self) -> bool {
        if !self.picker.as_ref().is_some_and(refs_loading) {
            return false;
        }
        self.compare_jobs.defer_confirm();
        true
    }

    /// Computes the merge base of HEAD and `rev` in the background and starts comparing.
    pub(super) fn choose_comparison(&mut self, label: String, rev: String, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { git::merge_base(&root, &rev) })
                .await;
            this.update(cx, |this, cx| match found {
                Ok((_, base, count)) => this.start_comparison(base, label, count, None, cx),
                Err(e) => this.notify_text(e, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Starts comparing with `base` (a full commit id: the merge base) as Before. `against` is
    /// what the reader chose, for the notice, which `prefix` may precede.
    pub(super) fn start_comparison(
        &mut self,
        base: String,
        against: String,
        merge_bases: usize,
        prefix: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mut text = comparing_notice(&against, &base, merge_bases);
        if let Some(p) = prefix {
            text = format!("{p} {text}");
        }
        let new = Comparison {
            base,
            against,
            merge_bases,
        };
        let fresh = self.compare.is_none();
        if crate::devlog::on() {
            eprintln!("COMPARE started base={}", cho_git::short(&new.base));
        }
        self.compare = Some(new.clone());
        if fresh {
            // A new comparison (Compare Against…, a pull request opening) shows what changed.
            self.reveal_changes_pane(true, cx);
        }
        let panes: Vec<PaneId> = self.editors.keys().copied().collect();
        for pane in panes {
            if self.ed_of(pane).side == Side::Before {
                // Stays on Before, at the new base and the same declaration.
                self.reload_before(pane, new.clone(), cx);
            } else {
                self.prepare_pane(pane, cx);
            }
        }
        self.refresh_uncommitted(cx);
        self.refresh_changes(false, cx);
        self.notify_text(text, cx);
    }

    // ---- Switching sides ----

    /// "Switch Before/After" (`b`): flips the window's side, and every code pane that is not
    /// half of a split goes to it (a split shows both sides already). Files opened afterwards
    /// open on that side.
    pub(super) fn switch_side(&mut self, cx: &mut Context<Self>) {
        let Some(cmp) = self.compare.clone() else {
            self.notify_text("Not comparing: use Compare Against\u{2026} first", cx);
            return;
        };
        let target = match self.fixed_side(self.active_editor) {
            Some(_) => self.window_side.flip(),
            None => self.ed().side.flip(),
        };
        self.window_side = target;
        let mut panes: Vec<PaneId> = self.editors.keys().copied().collect();
        panes.sort_by_key(|p| p.0);
        let mut moved = 0;
        for pane in panes {
            if self.fixed_side(pane).is_some() || self.ed_of(pane).doc.is_none() {
                continue;
            }
            moved += 1;
            // A pane off screen (in the tray, a hidden tab) flips when it is shown again, so it
            // comes back where it was (`apply_wanted_sides`).
            let shown = super::shell_host::pane_on_screen(self.shell.read(cx).workspace(), pane);
            if shown {
                self.switch_pane(pane, target, cmp.clone(), cx);
            } else if let Some(ed) = self.editors.get_mut(&pane) {
                ed.wanted_side = (ed.side != target).then_some(target);
            }
        }
        if moved == 0 {
            let side = match target {
                Side::After => "After",
                Side::Before => "Before",
            };
            self.notify_text(
                format!("Files open on {side}; the split shows both sides"),
                cx,
            );
        }
        self.dirty_all = true;
    }

    /// Flips the panes that `b` reached while they were off screen and are shown again (the
    /// layout changed: Tab from the tray, another tab, a dock shown).
    pub(super) fn apply_wanted_sides(&mut self, cx: &mut Context<Self>) {
        let Some(cmp) = self.compare.clone() else {
            return;
        };
        let waiting: Vec<(PaneId, Side)> = self
            .editors
            .iter()
            .filter_map(|(p, e)| e.wanted_side.map(|s| (*p, s)))
            .collect();
        for (pane, side) in waiting {
            if !super::shell_host::pane_on_screen(self.shell.read(cx).workspace(), pane) {
                continue;
            }
            if let Some(ed) = self.editors.get_mut(&pane) {
                ed.wanted_side = None;
            }
            self.switch_pane(pane, side, cmp.clone(), cx);
        }
    }

    /// The side `pane` shows for good, when it is half of a split.
    pub(super) fn fixed_side(&self, pane: PaneId) -> Option<Side> {
        self.links.iter().find(|l| l.has(pane)).map(|l| {
            if l.before == pane {
                Side::Before
            } else {
                Side::After
            }
        })
    }

    /// "Stop Comparing": every pane on Before goes back to After at the same declaration.
    pub(super) fn stop_comparing(&mut self, cx: &mut Context<Self>) {
        let Some(cmp) = self.compare.take() else {
            self.notify_text("Not comparing", cx);
            return;
        };
        self.links.clear();
        self.pending_link = None;
        self.window_side = Side::After;
        let panes: Vec<PaneId> = self.editors.keys().copied().collect();
        for pane in panes {
            if self.ed_of(pane).side == Side::Before {
                self.switch_pane(pane, Side::After, cmp.clone(), cx);
            } else if let Some(ed) = self.editors.get_mut(&pane) {
                ed.gutter = None;
                ed.prepared = None;
                ed.diff = None;
            }
            if let Some(ed) = self.editors.get_mut(&pane) {
                ed.wanted_side = None;
            }
        }
        self.uncommitted = None;
        self.refresh_changes(false, cx);
        self.notify_text("Stopped comparing", cx);
        self.dirty_all = true;
    }

    /// Puts `pane` on `target`. When the other side is prepared the swap is immediate; when it
    /// is not ready yet the current side stays on screen and the switch happens as soon as
    /// the other side is built.
    pub(super) fn switch_pane(
        &mut self,
        pane: PaneId,
        target: Side,
        cmp: Comparison,
        cx: &mut Context<Self>,
    ) {
        let ed = self.ed_of(pane);
        if ed.side == target || ed.doc.is_none() || ed.loading.is_some() {
            return;
        }
        let ready = ed
            .prepared
            .clone()
            .filter(|p| p.side == target && p.base == cmp.base && p.for_epoch == ed.epoch);
        match ready {
            Some(p) => match p.built.doc {
                Some(doc) => {
                    let l = Landing {
                        doc,
                        place: p.built.facts.diff.lines.clone(),
                        facts: p.built.facts,
                        reload: false,
                    };
                    self.land(pane, target, l, cmp, cx);
                }
                None => {
                    self.links = links_without(&self.links, pane);
                    self.dirty_all = true;
                    self.notify_text("Not in the merge base: this file is new", cx)
                }
            },
            None => self.load_and_land(pane, target, cmp, false, cx),
        }
    }

    /// Before again at a new base, same declaration; back to After (with the notice) only when
    /// the file is not at the new base.
    fn reload_before(&mut self, pane: PaneId, cmp: Comparison, cx: &mut Context<Self>) {
        self.load_and_land(pane, Side::Before, cmp, true, cx);
    }

    /// Builds `target` in the background, then lands it (the not-prepared path, and the reload).
    fn load_and_land(
        &mut self,
        pane: PaneId,
        target: Side,
        cmp: Comparison,
        reload: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let ed = self.ed_of(pane);
        let (Some(old), None) = (ed.doc.clone(), &ed.loading) else {
            return;
        };
        self.with_pane(pane, |this| this.ed_mut().generation.next());
        let (generation, epoch) = (self.ed_of(pane).generation, self.ed_of(pane).epoch);
        let theme = self.theme.clone();
        let json = self.features.on(Feature::JsonFolds);
        let path = old.path.clone();
        let display = self.display_of(&path);
        let build_cmp = cmp.clone();
        cx.spawn(async move |this, cx| {
            let built = cx
                .background_spawn(async move {
                    let built =
                        build_other(&root, &build_cmp.base, &path, target, display, &theme, json)?;
                    let doc = built
                        .doc
                        .clone()
                        .ok_or("Not in the merge base: this file is new")?;
                    // A reload maps the caret from the old Before to the new Before.
                    let place = if reload {
                        Arc::new(
                            diff_lines(
                                old.buffer.text().as_bytes(),
                                doc.buffer.text().as_bytes(),
                                DEFAULT_MAX_COST,
                            )
                            .hunks,
                        )
                    } else {
                        built.facts.diff.lines.clone()
                    };
                    Ok::<_, String>(Landing {
                        doc,
                        facts: built.facts,
                        place,
                        reload,
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                let current = this
                    .editors
                    .get(&pane)
                    .is_some_and(|e| e.generation.is(generation) && e.epoch == epoch);
                if !current {
                    return;
                }
                match built {
                    Ok(l) => this.land(pane, target, l, cmp, cx),
                    Err(e) => {
                        this.links = links_without(&this.links, pane);
                        // Both columns' headers lose their chain.
                        this.dirty_all = true;
                        this.notify_text(e, cx);
                        if reload {
                            this.switch_pane(pane, Side::After, cmp, cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Installs the other side's document in `pane`, keeping the place by declaration and the
    /// column, and keeps the side just left as the prepared one. Runs on the UI thread and does
    /// no file reads, diffs or parsing.
    fn land(
        &mut self,
        pane: PaneId,
        target: Side,
        l: Landing,
        cmp: Comparison,
        cx: &mut Context<Self>,
    ) {
        let started = Instant::now();
        let ed = self.ed_of(pane);
        let Some(old) = ed.doc.clone() else { return };
        let (from_side, old_index_ok, caret) = (ed.side, ed.rev_index_ok, ed.caret);
        // The Changes list asked for a declaration on this side: it decides the place.
        let by_change = ed.pending_place.as_ref().is_some_and(|p| p.side == target);
        // The declaration map of the document on screen is kept with it.
        let old_decls = ed.decls.clone().unwrap_or_else(|| Arc::new(decls(&old)));
        // Where the view sits now, to keep it there on the other side: the caret's screen row,
        // or (when the caret is out of view) the top line mapped through the diff.
        let anchor = ed.anchor().map(|mut a| {
            if let Anchor::Top { line, .. } = &mut a.at {
                *line = map_line(&l.place, *line, from_side);
            }
            a
        });
        let caret_line = caret.map_or(0, |c| old.buffer.line_of(c));
        let col = caret.map_or(0, |c| {
            c - old.buffer.line_range(caret_line).map_or(0, |r| r.start)
        });
        let placed = keep_place(&old_decls, &l.facts.decls, caret_line, &l.place, from_side);
        let still = self.compare.as_ref().map(|c| &c.base) == Some(&cmp.base);
        let line_count = l.doc.lines.len();
        let (line, note) = match &placed {
            Placed::Line(line) => (*line, None),
            Placed::Only { name, line, added } => (*line, Some((name.clone(), *added))),
        };
        let byte = l
            .doc
            .buffer
            .line_range(line.min(line_count.saturating_sub(1)))
            .map(|r| byte_in_line(l.doc.buffer.text(), r, col));
        let doc = l.doc.clone();
        // A pane in a linked pair shows one side for good: it keeps no prepared other side.
        let paired = self.links.iter().any(|l| l.has(pane));
        self.with_pane(pane, |this| {
            this.install_arc(doc);
            let ed = this.ed_mut();
            ed.side = target;
            ed.rev_index_ok = l.facts.index_ok;
            ed.decls = Some(l.facts.decls.clone());
            ed.gutter = still
                .then(|| Arc::new(Gutter::from_hunks(&l.facts.diff.hunks, target, line_count)));
            // The side just left is now the prepared one: nothing to rebuild.
            ed.diff = still.then(|| l.facts.diff.clone());
            ed.prepared = (still && from_side != target && !paired).then(|| Prepared {
                side: from_side,
                base: cmp.base.clone(),
                for_epoch: ed.epoch,
                built: Built {
                    doc: Some(old),
                    facts: SideFacts {
                        decls: old_decls,
                        diff: l.facts.diff.clone(),
                        index_ok: old_index_ok,
                    },
                },
            });
            if let Some(b) = byte {
                let reveal = anchor.map_or(Reveal::Center, Reveal::Keep);
                this.place_caret(b, reveal, cx);
            }
            if by_change && let Some(place) = this.ed_mut().pending_place.take() {
                this.apply_place(&place, cx);
            }
            if target == Side::After {
                this.refresh_call_kinds(cx);
            }
        });
        if still && from_side == target {
            self.prepare_pane(pane, cx);
        }
        if let Some((name, added)) = note.filter(|_| !by_change) {
            let text = match (l.reload, added) {
                (true, _) => format!("`{name}` does not exist at the new merge base"),
                (false, true) => format!("`{name}` was added in this change"),
                (false, false) => format!("`{name}` is removed in this change"),
            };
            self.notify_text(text, cx);
        } else if l.facts.diff.coarse && still {
            self.notify_text("Diff too large; shown as one change", cx);
        }
        self.sync_editor_pane(pane, cx);
        self.refresh_context(cx);
        self.dirty_pane = Some(pane);
        self.sync_pair(pane, cx);
        if crate::devlog::on() {
            eprintln!("SWITCH ui-apply={:?}", started.elapsed());
        }
    }

    // ---- Linked split ----

    /// "Compare Side by Side": splits the active pane to the right, Before on the left and After
    /// on the right, linked by declaration.
    pub(super) fn compare_side_by_side(&mut self, cx: &mut Context<Self>) {
        let Some(cmp) = self.compare.clone() else {
            self.notify_text("Not comparing: use Compare Against\u{2026} first", cx);
            return;
        };
        let pane = self.active_editor;
        if self.links.iter().any(|l| l.has(pane)) {
            // Already half of a pair: just bring it to the front.
            self.shell.update(cx, |shell, cx| {
                let _ = shell.update_workspace(cx, |ws| ws.activate(pane));
            });
            return;
        }
        if self.ed().doc.is_none() || self.ed().loading.is_some() {
            self.notify_text("Open a file first", cx);
            return;
        }
        let added = self.shell.update(cx, |shell, cx| {
            shell.update_workspace(cx, |ws| ws.split(pane, cho_layout::Edge::Right))
        });
        let right = match added {
            Ok(p) => p,
            Err(e) => {
                self.notify_text(e.to_string(), cx);
                return;
            }
        };
        self.sync_panes(cx);
        if !self.editors.contains_key(&right) {
            return;
        }
        self.links.push(SplitLink {
            before: pane,
            after: right,
            linked: true,
        });
        // Left shows Before for good, right shows After: put each there. Neither keeps the
        // other side prepared.
        let left_side = self.ed_of(pane).side;
        let right_side = self.ed_of(right).side;
        if left_side == Side::After {
            self.switch_pane(pane, Side::Before, cmp.clone(), cx);
        }
        if right_side == Side::Before {
            self.switch_pane(right, Side::After, cmp, cx);
        }
        for p in [pane, right] {
            if let Some(ed) = self.editors.get_mut(&p) {
                ed.prepared = None;
                ed.last_decl = None;
            }
        }
        self.dirty_all = true;
    }

    /// "Link Split" / "Unlink Split": whether the pair follows each other's declaration.
    pub(super) fn toggle_split_link(&mut self, cx: &mut Context<Self>) {
        let pane = self.active_editor;
        let Some(i) = self.links.iter().position(|l| l.has(pane)) else {
            self.notify_text(
                "This pane is not half of a split: use Compare Side by Side",
                cx,
            );
            return;
        };
        self.links[i].linked = !self.links[i].linked;
        let linked = self.links[i].linked;
        if linked {
            // Catch up: the other pane goes to the declaration under this caret now.
            self.ed_mut().last_decl = None;
            self.follow_link(cx);
        }
        self.notify_text(
            if linked {
                "Split linked"
            } else {
                "Split unlinked"
            },
            cx,
        );
        self.dirty_all = true;
    }

    /// Ends the pair `pane` is in, if any; both panes go back to keeping their other side
    /// prepared.
    pub(super) fn unlink(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let Some(link) = self.links.iter().copied().find(|l| l.has(pane)) else {
            return;
        };
        self.links = links_without(&self.links, pane);
        for p in [link.before, link.after] {
            if self.compare.is_some() && self.editors.contains_key(&p) && p != pane {
                self.prepare_pane(p, cx);
            }
        }
        self.dirty_all = true;
    }

    /// The caret of the active pane moved: when it is half of a linked pair and the declaration
    /// around it changed since the last look, the other pane goes to the same declaration (same
    /// offset inside it, same column), revealed, without taking focus. Scrolling alone does not
    /// link. Uses the cached declaration maps and diff: no reads, no parsing, no diffing.
    /// Returns whether the other pane was moved.
    pub(super) fn follow_link(&mut self, cx: &mut Context<Self>) -> bool {
        if self.following {
            return false;
        }
        let pane = self.active_editor;
        let Some(link) = self.links.iter().copied().find(|l| l.linked && l.has(pane)) else {
            return false;
        };
        let started = Instant::now();
        let other = link.other(pane);
        self.fill_decls(pane);
        self.fill_decls(other);
        // Both panes must show their own side and have what the follow reads.
        let (Some(me), Some(them)) = (self.editors.get(&pane), self.editors.get(&other)) else {
            return false;
        };
        if me.side == them.side || me.loading.is_some() || them.loading.is_some() {
            return false;
        }
        let (Some(doc), Some(caret), Some(odoc)) = (&me.doc, me.caret, &them.doc) else {
            return false;
        };
        let (Some(my_decls), Some(their_decls), Some(diff)) = (&me.decls, &them.decls, &me.diff)
        else {
            // The diff is not ready yet (still being prepared): try on the next move.
            return false;
        };
        let line = doc.buffer.line_of(caret);
        let now = innermost_index(my_decls, line);
        if !declaration_changed(me.last_decl, now) {
            return false;
        }
        let col = caret - doc.buffer.line_range(line).map_or(0, |r| r.start);
        let from_side = me.side;
        let placed = keep_place(my_decls, their_decls, line, &diff.lines, from_side);
        let target_line = match placed {
            Placed::Line(l) | Placed::Only { line: l, .. } => l,
        };
        let target_line = target_line.min(odoc.lines.len().saturating_sub(1));
        let byte = odoc
            .buffer
            .line_range(target_line)
            .map(|r| byte_in_line(odoc.buffer.text(), r, col));
        let their_now = innermost_index(their_decls, target_line);
        // The declaration it lands on is highlighted briefly: its first line, else the line.
        let flash_line = their_now
            .and_then(|i| their_decls.get(i))
            .map_or(target_line, |d| d.first_line);
        if let Some(ed) = self.editors.get_mut(&pane) {
            ed.last_decl = Some(now);
        }
        if let Some(b) = byte {
            // The followed pane scrolls only when the declaration is out of view.
            self.following = true;
            self.with_pane(other, |this| {
                this.place_caret(b, Reveal::IfHidden, cx);
            });
            self.following = false;
            if let Some(ed) = self.editors.get_mut(&other) {
                ed.last_decl = Some(their_now);
            }
            self.start_flash(other, flash_line, cx);
            self.shell
                .update(cx, |shell, cx| shell.refresh_pane(other, cx));
        }
        if crate::devlog::on() {
            eprintln!("FOLLOW us={}", started.elapsed().as_micros());
        }
        byte.is_some()
    }

    /// " · linked" or " · unlinked" when the active pane is half of a split, else nothing.
    pub(super) fn link_label(&self) -> String {
        match self.links.iter().find(|l| l.has(self.active_editor)) {
            Some(l) if l.linked => " \u{b7} linked".to_string(),
            Some(_) => " \u{b7} unlinked".to_string(),
            None => String::new(),
        }
    }

    /// Builds `pane`'s declaration map when it has none.
    fn fill_decls(&mut self, pane: PaneId) {
        if let Some(ed) = self.editors.get_mut(&pane)
            && ed.decls.is_none()
            && let Some(doc) = &ed.doc
        {
            ed.decls = Some(Arc::new(decls(doc)));
        }
    }

    // ---- Opening from the Changes list ----

    /// Opens `place` (a file at `abs`) in the active pane: the working file at the declaration
    /// or line on After, or the same file switched to Before. A deleted file has only Before.
    /// After lands in the history; Before does not (Back would restore a Before offset in the
    /// After file).
    pub(super) fn open_place(&mut self, abs: PathBuf, place: ChangePlace, cx: &mut Context<Self>) {
        if place.deleted {
            self.open_deleted(abs, place, cx);
            return;
        }
        let record = place.side == Side::After;
        let paired = self.fixed_side(self.active_editor).is_some();
        let ed = self.ed();
        // The file on screen: the place is applied here (in a split, the column on its side
        // shows it).
        let here = ed.loading.is_none()
            && (ed.side == place.side || paired)
            && ed.doc.as_ref().is_some_and(|d| d.path == abs);
        if here {
            if record {
                self.remember_caret();
            }
            self.apply_place(&place, cx);
            if record {
                self.record_history();
            }
            return;
        }
        let target = Target {
            place: Some(place),
            ..Target::default()
        };
        self.open(abs, target, record, cx);
    }

    /// Shows a file that exists only at the merge base (deleted since), read from the object
    /// store, on Before.
    fn open_deleted(&mut self, abs: PathBuf, place: ChangePlace, cx: &mut Context<Self>) {
        let (Some(cmp), Some(root)) = (self.compare.clone(), self.root.clone()) else {
            return;
        };
        let pane = self.active_editor;
        self.unlink(pane, cx);
        self.with_pane(pane, |this| {
            this.remember_caret();
            let ed = this.ed_mut();
            ed.generation.next();
            ed.pending_place = None;
            ed.loading = Some(abs.clone());
            ed.error = None;
        });
        let generation = self.ed_of(pane).generation;
        let display = self.display_of(&abs);
        let theme = self.theme.clone();
        let json = self.features.on(Feature::JsonFolds);
        cx.spawn(async move |this, cx| {
            let (rel, base) = (place.path.clone(), cmp.base.clone());
            let path = abs.clone();
            let built = cx
                .background_spawn(async move {
                    let bytes = git::base_blob(&root, &base, &rel)?;
                    Document::from_bytes(
                        path,
                        display,
                        bytes,
                        Some(cho_git::short(&base)),
                        &theme,
                        json,
                    )
                    .map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                if !this.editor_is(pane, generation) {
                    return;
                }
                this.with_pane(pane, |this| {
                    this.ed_mut().loading = None;
                    match built {
                        Ok(doc) => {
                            this.install(doc);
                            this.ed_mut().side = Side::Before;
                            this.apply_place(&place, cx);
                        }
                        Err(e) => this.ed_mut().error = Some(e),
                    }
                });
                this.sync_editor_pane(pane, cx);
                this.refresh_context(cx);
                this.dirty_pane = Some(pane);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Puts the active pane at `place`. On the right side it moves the caret there; on the
    /// other it switches sides first and lands there (`land`).
    pub(super) fn apply_place(&mut self, place: &ChangePlace, cx: &mut Context<Self>) {
        if self.ed().doc.is_none() {
            return;
        }
        if self.ed().side == place.side {
            match self.resolve_place(place) {
                Ok(byte) => self.set_caret(byte, true, cx),
                Err(text) => self.notify_text(text, cx),
            }
            return;
        }
        let Some(cmp) = self.compare.clone() else {
            return;
        };
        let pane = self.active_editor;
        match self.fixed_side(pane) {
            // Half of a split, on the other side from the place: the partner column shows it,
            // and this one follows.
            Some(fixed) if fixed != place.side => {
                let Some(other) = self
                    .links
                    .iter()
                    .find(|l| l.has(pane))
                    .map(|l| l.other(pane))
                else {
                    return;
                };
                let lands = self.ed_of(other).doc.is_some() && self.ed_of(other).loading.is_none();
                if lands {
                    self.with_pane(other, |this| {
                        this.apply_place(place, cx);
                        this.ed_mut().last_decl = None;
                        this.follow_link(cx);
                    });
                }
                return;
            }
            // Half of a split that is still getting to its own side: land there.
            Some(_) => {}
            None => self.unlink(pane, cx),
        }
        self.ed_mut().pending_place = Some(place.clone());
        self.switch_pane(pane, place.side, cmp, cx);
    }

    /// A column of a pair that opened a new file and holds the place: once both columns are
    /// ready, the partner follows it.
    pub(super) fn sync_pair(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let Some(link) = self.links.iter().copied().find(|l| l.has(pane)) else {
            return;
        };
        for p in [link.before, link.after] {
            let ready = self
                .editors
                .get(&p)
                .is_some_and(|e| e.lead_pair && e.caret.is_some());
            if !ready {
                continue;
            }
            let followed = self.with_pane(p, |this| {
                this.ed_mut().last_decl = None;
                this.follow_link(cx)
            });
            if followed && let Some(ed) = self.editors.get_mut(&p) {
                ed.lead_pair = false;
            }
        }
    }

    /// The byte to land on: the declaration `place.key` names in the document on screen (the
    /// `nth` of those sharing the key), else the line. A declaration that is not there is
    /// reported, never replaced by a similar one.
    fn resolve_place(&self, place: &ChangePlace) -> Result<usize, String> {
        let doc = self.ed().doc.as_ref().ok_or("No file is open")?;
        let side = match place.side {
            Side::After => "After",
            Side::Before => "Before",
        };
        if let Some(key) = &place.key {
            let found = doc
                .language
                .map(|lang| matching_symbols(&doc.symbols, lang, key))
                .and_then(|hits| hits.get(place.nth).copied());
            return match found {
                Some(i) => Ok(doc.symbols[i].name_range.start),
                None => Err(format!(
                    "`{}` not found in {} on {side}",
                    key.qualified_name, doc.display_path
                )),
            };
        }
        let line = place
            .line
            .unwrap_or(0)
            .min(doc.lines.len().saturating_sub(1));
        Ok(doc.buffer.line_range(line).map_or(0, |r| r.start))
    }

    // ---- Uncommitted work ----

    /// The line of the working copy that matches `line` of the document on screen: the same
    /// line on After, the diff-mapped one on Before (when the working side is prepared).
    pub(super) fn working_line(&self, line: usize) -> usize {
        let ed = self.ed();
        match (&self.compare, ed.side, &ed.diff) {
            (Some(_), Side::Before, Some(d)) => map_line(&d.lines, line, Side::Before),
            _ => line,
        }
    }

    /// Counts the files that differ between HEAD and the working folder, in the background
    /// (when comparing). Asked when a comparison starts, when the window is activated and after
    /// a file opens; never polled.
    pub(super) fn refresh_uncommitted(&mut self, cx: &mut Context<Self>) {
        let (Some(_), Some(root)) = (&self.compare, self.root.clone()) else {
            self.uncommitted = None;
            return;
        };
        let generation = self.uncommitted_gen.next();
        cx.spawn(async move |this, cx| {
            let count = cx
                .background_spawn(async move {
                    let top = git::repo_top(&root).ok()?;
                    cho_changes::uncommitted_files(&top).ok().map(|f| f.len())
                })
                .await;
            this.update(cx, |this, cx| {
                if this.uncommitted_gen.is(generation) && this.compare.is_some() {
                    this.uncommitted = count;
                    this.dirty_all = true;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- Preparing the other side ----

    /// Builds the gutter marks and the other side of `pane`'s file in the background (when
    /// comparing). Called when a document loads or the comparison starts, never per frame;
    /// afterwards switching sides is a swap.
    pub(super) fn prepare_pane(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let (Some(cmp), Some(root)) = (self.compare.clone(), self.root.clone()) else {
            if let Some(ed) = self.editors.get_mut(&pane) {
                ed.gutter = None;
                ed.prepared = None;
            }
            return;
        };
        let ed = self.ed_of(pane);
        let Some(doc) = ed.doc.clone() else { return };
        let (side, epoch) = (ed.side, ed.epoch);
        let path = doc.path.clone();
        let display = self.display_of(&path);
        let theme = self.theme.clone();
        let json = self.features.on(Feature::JsonFolds);
        cx.spawn(async move |this, cx| {
            let base = cmp.base.clone();
            let built = cx
                .background_spawn(async move {
                    build_other(&root, &cmp.base, &path, side.flip(), display, &theme, json).ok()
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(built) = built else {
                    this.apply_pending_link(pane, false, cx);
                    return;
                };
                let same = this.compare.as_ref().map(|c| &c.base) == Some(&base);
                let paired = this.links.iter().any(|l| l.has(pane));
                let Some(ed) = this.editors.get_mut(&pane) else {
                    return;
                };
                if !same || ed.epoch != epoch || ed.side != side {
                    return;
                }
                let lines = ed.doc.as_ref().map_or(0, |d| d.lines.len());
                let diff = &built.facts.diff;
                ed.gutter = Some(Arc::new(Gutter::from_hunks(&diff.hunks, side, lines)));
                if ed.decls.is_none() {
                    ed.decls = ed.doc.as_ref().map(|d| Arc::new(decls(d)));
                }
                ed.diff = Some(diff.clone());
                ed.prepared = (!paired).then(|| Prepared {
                    side: side.flip(),
                    base,
                    for_epoch: epoch,
                    built,
                });
                this.dirty_pane = Some(pane);
                this.apply_pending_link(pane, true, cx);
                this.sync_pair(pane, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        CompareAgainst,
        CompareSideBySide,
        NextUncommitted,
        PrevUncommitted,
        StopComparing,
        SwitchBeforeAfter,
        ToggleHighlightUncommitted,
        ToggleSplitLink
    ]
);

pub(super) const COMMANDS: &[Command] = &[
    c(
        "chorong::CompareAgainst",
        "Compare Against\u{2026}",
        Layer::Compare,
        168,
    ),
    c(
        "chorong::CompareSideBySide",
        "Compare Side by Side",
        Layer::Compare,
        165,
    ),
    c(
        "chorong::StopComparing",
        "Stop Comparing",
        Layer::Compare,
        169,
    ),
    c(
        "chorong::ToggleSplitLink",
        "Link or Unlink Split",
        Layer::Compare,
        306,
    ),
    c(
        "chorong::SwitchBeforeAfter",
        "Switch Before/After",
        Layer::Compare,
        305,
    ),
    c(
        "chorong::NextUncommitted",
        "Next Uncommitted Change",
        Layer::Compare,
        309,
    ),
    c(
        "chorong::PrevUncommitted",
        "Previous Uncommitted Change",
        Layer::Compare,
        311,
    ),
    c(
        "chorong::ToggleHighlightUncommitted",
        "Highlight Uncommitted",
        Layer::Compare,
        312,
    ),
];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, CompareAgainst, |this, _w, cx| this
        .open_compare_picker(cx));
    let d = on!(d, cx, NextUncommitted, |this, _w, cx| this
        .step_uncommitted(true, cx));
    let d = on!(d, cx, PrevUncommitted, |this, _w, cx| this
        .step_uncommitted(false, cx));
    let d = on!(d, cx, ToggleHighlightUncommitted, |this, _w, cx| this
        .toggle_highlight_uncommitted(cx));
    let d = on!(d, cx, CompareSideBySide, |this, _w, cx| this
        .compare_side_by_side(cx));
    let d = on!(d, cx, ToggleSplitLink, |this, _w, cx| this
        .toggle_split_link(cx));
    let d = on!(d, cx, StopComparing, |this, _w, cx| this.stop_comparing(cx));
    on!(d, cx, SwitchBeforeAfter, |this, _w, cx| this
        .switch_side(cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cho_git::{RefEntry, RefKind};

    fn compare_picker(query: &str) -> PickerState {
        let (label, hint, pick) = loading_row();
        PickerState {
            kind: PickerKind::Compare,
            query: query.into(),
            query_all: false,
            add_to_page: false,
            matches: Vec::new(),
            selected: 0,
            labels: vec![label],
            hints: vec![hint],
            picks: vec![pick],
            tour_scope: false,
            peek_open: false,
        }
    }

    fn row(label: &str, hint: &str, rev: &str) -> (String, String, String) {
        (label.into(), hint.into(), rev.into())
    }

    #[test]
    fn enter_while_listing_is_held_for_that_listing_only() {
        let mut jobs = PickerJobs::default();
        let listing = jobs.next_refs();
        assert!(refs_loading(&compare_picker("main")));
        jobs.defer_confirm();
        assert!(jobs.take_confirm(listing), "applied when its list lands");
        assert!(!jobs.take_confirm(listing), "once");
        // Held for a listing the picker was reopened since: the new listing does not apply it.
        let old = jobs.next_refs();
        jobs.defer_confirm();
        let new = jobs.next_refs();
        assert!(!jobs.take_confirm(new));
        assert!(
            !jobs.take_confirm(old),
            "taken and dropped by the newer landing"
        );
    }

    #[test]
    fn once_listed_enter_picks_the_selected_ref() {
        let mut p = compare_picker("main");
        fill_ref_rows(
            &mut p,
            vec![row("main", "branch", "m"), row("v1", "tag", "t")],
        );
        assert!(!refs_loading(&p), "the list landed");
        p.matches = vec![crate::picker::Match {
            index: 0,
            score: 1,
            ranges: Vec::new(),
        }];
        assert_eq!(selected_ref(&p), Some(("main".into(), "m".into())));
        p.matches.clear();
        assert_eq!(selected_ref(&p), None, "no match: nothing is chosen");
    }

    #[test]
    fn only_the_newest_lookup_of_each_kind_lands() {
        let mut jobs = PickerJobs::default();
        let first = jobs.next_commit();
        let second = jobs.next_commit();
        assert!(
            !jobs.commit_current(first),
            "an older keystroke's answer drops"
        );
        assert!(jobs.commit_current(second));
        // Opening the picker again does not disturb a typed-commit lookup, and vice versa.
        let refs = jobs.next_refs();
        assert!(jobs.commit_current(second) && jobs.refs_current(refs));
        let reopened = jobs.next_refs();
        assert!(!jobs.refs_current(refs) && jobs.refs_current(reopened));
    }

    #[test]
    fn a_commit_answer_lands_only_for_the_query_it_was_asked_for() {
        let mut p = compare_picker("abc1234");
        assert!(!land_commit_row(&mut p, "abc12", Some("abc12full".into())));
        assert_eq!(p.labels.len(), 1, "a stale answer adds no row");
        assert!(
            !land_commit_row(&mut p, "abc1234", None),
            "no such commit: no row"
        );
        assert!(land_commit_row(
            &mut p,
            "abc1234",
            Some("abc1234full".into())
        ));
        assert_eq!(p.hints.last().map(String::as_str), Some(COMMIT_HINT));
        assert!(matches!(p.picks.last(), Some(Pick::Ref { rev, .. }) if rev == "abc1234full"));
        // Landing again replaces the row rather than adding a second one.
        assert!(land_commit_row(
            &mut p,
            "abc1234",
            Some("abc1234full".into())
        ));
        assert_eq!(p.hints.iter().filter(|h| *h == COMMIT_HINT).count(), 1);
    }

    #[test]
    fn a_commit_answer_does_not_land_on_another_picker() {
        let mut p = compare_picker("abc1234");
        p.kind = PickerKind::Palette;
        assert!(!land_commit_row(&mut p, "abc1234", Some("full".into())));
        assert_eq!(p.labels.len(), 1);
    }

    #[test]
    fn refs_replace_the_loading_row_and_keep_a_typed_commit_last() {
        let mut p = compare_picker("abc1234");
        assert!(land_commit_row(&mut p, "abc1234", Some("full".into())));
        fill_ref_rows(
            &mut p,
            vec![row("main", "branch", "m"), row("v1", "tag", "t")],
        );
        assert_eq!(p.labels, ["main", "v1", "abc1234"]);
        assert_eq!(p.hints, ["branch", "tag", COMMIT_HINT]);
        assert!(!p.hints.iter().any(|h| h == LOADING_HINT));
        assert_eq!(p.picks.len(), 3);
        // A new query clears the typed-commit row and keeps the refs.
        set_commit_row(&mut p, None);
        assert_eq!(p.labels, ["main", "v1"]);
    }

    fn h(old: std::ops::Range<usize>, new: std::ops::Range<usize>) -> LineHunk {
        LineHunk {
            base: old,
            head: new,
        }
    }

    fn d(name: &str, ordinal: usize, first: usize, last: usize) -> Decl {
        Decl {
            name: name.into(),
            kind: SymbolKind::Function,
            ordinal,
            first_line: first,
            last_line: last,
        }
    }

    fn fh(hunk: LineHunk, uncommitted: bool) -> FileHunk {
        FileHunk { hunk, uncommitted }
    }

    fn committed(hunks: &[LineHunk]) -> Vec<FileHunk> {
        hunks.iter().map(|h| fh(h.clone(), false)).collect()
    }

    #[test]
    fn gutter_marks_this_sides_lines_and_wedges_where_the_other_side_has_more() {
        // Before has line 2 removed (After lacks it) and After has lines 3..5 added.
        let hunks = committed(&[h(2..3, 2..2), h(4..4, 3..5)]);
        let after = Gutter::from_hunks(&hunks, Side::After, 6);
        assert!(after.wedge_above(2) && !after.changed(2));
        assert!(after.changed(3) && after.changed(4) && !after.changed(5));
        let before = Gutter::from_hunks(&hunks, Side::Before, 6);
        assert!(before.changed(2) && !before.changed(3));
        assert!(before.wedge_above(4));
        let end = Gutter::from_hunks(&committed(&[h(0..0, 6..6)]), Side::Before, 6);
        assert!(end.wedge_at_end() || end.wedge_above(0));
    }

    #[test]
    fn uncommitted_marks_are_hollow_and_gaps_say_what_is_missing() {
        // Line 1 committed; line 3 only uncommitted; a committed deletion at 5, an
        // uncommitted one at 7; lines 9 changed by both pieces side by side.
        let hunks = [
            fh(h(1..2, 1..2), false),
            fh(h(3..4, 3..4), true),
            fh(h(5..6, 5..5), false),
            fh(h(7..8, 6..6), true),
            fh(h(9..10, 7..8), false),
            fh(h(10..11, 8..9), true),
        ];
        let after = Gutter::from_hunks(&hunks, Side::After, 12);
        assert!(after.changed(1) && !after.hollow(1));
        assert!(after.hollow(3) && !after.changed(3));
        assert!(after.wedge_above(5) && !after.wedge_is_hollow(5));
        assert!(after.wedge_above(6) && after.wedge_is_hollow(6));
        assert!(after.changed(7) && after.hollow(8));
        // Before marks lines uncommitted work removed or changed apart too.
        let before = Gutter::from_hunks(&hunks, Side::Before, 12);
        assert!(before.changed(1) && !before.hollow(1));
        assert!(before.hollow(3) && !before.changed(3));
        assert!(before.hollow(7) && before.changed(5));
        // The gaps say what the other side has there.
        assert_eq!(
            after.gap(6),
            Some(&Gap {
                line: 6,
                count: 1,
                other: 7,
                uncommitted: true
            })
        );
        assert_eq!(
            after.gap(5).map(|g| (g.count, g.uncommitted)),
            Some((1, false))
        );
        assert_eq!(after.gap(4), None);
        assert_eq!(
            gap_text(Side::After, after.gap(5).unwrap()),
            "1 line removed here \u{b7} click to see Before"
        );
        // A committed and an uncommitted wedge at one position: the committed one wins.
        let both = [fh(h(1..2, 4..4), true), fh(h(5..6, 4..4), false)];
        let g = Gutter::from_hunks(&both, Side::After, 6);
        assert!(g.wedge_above(4) && !g.wedge_is_hollow(4));
        let swapped = [fh(h(5..6, 4..4), false), fh(h(1..2, 4..4), true)];
        assert_eq!(g, Gutter::from_hunks(&swapped, Side::After, 6));
    }

    #[test]
    fn uncommitted_runs_count_line_runs_and_deletions() {
        // Lines 1-2 uncommitted, line 4 committed, uncommitted deletions above lines 6 and 8,
        // and line 7 uncommitted.
        let hunks = [
            fh(h(1..3, 1..3), true),
            fh(h(4..5, 4..5), false),
            fh(h(6..7, 6..6), true),
            fh(h(8..8, 7..8), true),
            fh(h(9..10, 8..8), true),
        ];
        let g = Gutter::from_hunks(&hunks, Side::After, 10);
        assert_eq!(g.uncommitted_runs(), [1, 6, 7, 8]);
        let none = Gutter::from_hunks(&committed(&[h(1..2, 1..2)]), Side::After, 4);
        assert!(none.uncommitted_runs().is_empty());
    }

    /// The next (or previous) start after (before) `line`, wrapping round.
    #[test]
    fn run_steps_wrap() {
        let runs = [3, 10, 20];
        assert_eq!(next_run(&runs, Some(5), true), Some(10));
        assert_eq!(next_run(&runs, Some(20), true), Some(3));
        assert_eq!(next_run(&runs, Some(3), false), Some(20));
        assert_eq!(next_run(&runs, None, true), Some(3));
        assert_eq!(next_run(&[], Some(1), true), None);
    }

    #[test]
    fn changed_words_are_found_per_hunk_on_both_sides() {
        let base = "a\n    format!(\"s-{user}\")\nb\nif ok { run() }\n";
        let head = "a\n    format!(\"session-{user}\")\nb\n    if ok { run() }\n";
        let hunks = committed(&[h(1..2, 1..2), h(3..4, 3..4)]);
        let w = Words::compute(base.as_bytes(), head.as_bytes(), &hunks);
        let line = |text: &str, n: usize| text.lines().nth(n).unwrap().to_string();
        let r = &w.on(Side::Before, 1)[0];
        assert_eq!(&line(base, 1)[r.clone()], "s");
        let r = &w.on(Side::After, 1)[0];
        assert_eq!(&line(head, 1)[r.clone()], "session");
        assert!(w.whitespace_only(Side::After, 3) && w.whitespace_only(Side::Before, 3));
        assert!(w.on(Side::After, 3).is_empty() && w.on(Side::After, 0).is_empty());
    }

    #[test]
    fn lines_map_through_the_diff() {
        // Two lines were added at line 2 of After; one line changed at Before line 6.
        let hunks = [h(2..2, 2..4), h(6..7, 8..9)];
        assert_eq!(map_line(&hunks, 1, Side::Before), 1);
        assert_eq!(map_line(&hunks, 2, Side::Before), 4);
        assert_eq!(map_line(&hunks, 6, Side::Before), 8);
        assert_eq!(map_line(&hunks, 9, Side::Before), 11);
        assert_eq!(map_line(&hunks, 3, Side::After), 2);
        assert_eq!(map_line(&hunks, 8, Side::After), 6);
        assert_eq!(map_line(&hunks, 10, Side::After), 8);
    }

    #[test]
    fn the_caret_keeps_its_offset_inside_the_same_declaration() {
        // `f` starts at line 10 on Before and line 14 on After, and grew by two lines.
        let before = [d("f", 0, 10, 14), d("g", 0, 20, 25)];
        let after = [d("f", 0, 14, 20), d("g", 0, 30, 35)];
        let hunks = [h(0..0, 0..4)];
        assert_eq!(
            keep_place(&before, &after, 12, &hunks, Side::Before),
            Placed::Line(16)
        );
        // Offset past the other side's end is clamped.
        assert_eq!(
            keep_place(&after, &before, 20, &hunks, Side::After),
            Placed::Line(14)
        );
    }

    #[test]
    fn the_innermost_declaration_and_the_nth_duplicate_are_used() {
        let outer = d("S", 0, 0, 20);
        let inner = d("S::m", 0, 5, 10);
        let before = [outer.clone(), inner.clone()];
        let after = [d("S", 0, 0, 30), d("S::m", 0, 15, 20)];
        assert_eq!(
            keep_place(&before, &after, 6, &[], Side::Before),
            Placed::Line(16)
        );
        // Overloads with the same name pair up by order.
        let a = [d("f", 0, 0, 3), d("f", 1, 5, 8)];
        let b = [d("f", 0, 10, 13), d("f", 1, 15, 18)];
        assert_eq!(keep_place(&a, &b, 6, &[], Side::Before), Placed::Line(16));
    }

    #[test]
    fn a_declaration_on_one_side_only_maps_through_the_diff() {
        let after = [d("added", 0, 3, 5)];
        let hunks = [h(3..3, 3..6)];
        assert_eq!(
            keep_place(&after, &[], 4, &hunks, Side::After),
            Placed::Only {
                name: "added".into(),
                line: 3,
                added: true
            }
        );
        let before = [d("gone", 0, 3, 5)];
        let hunks = [h(3..6, 3..3)];
        assert_eq!(
            keep_place(&before, &[], 4, &hunks, Side::Before),
            Placed::Only {
                name: "gone".into(),
                line: 3,
                added: false
            }
        );
        // No declaration around the caret: just the mapped line.
        assert_eq!(
            keep_place(&[], &[], 10, &[h(0..0, 0..2)], Side::Before),
            Placed::Line(12)
        );
    }

    #[test]
    fn the_innermost_index_and_the_follow_decision() {
        let ds = [d("S", 0, 0, 20), d("S::m", 0, 5, 10), d("t", 0, 22, 25)];
        assert_eq!(innermost_index(&ds, 7), Some(1));
        assert_eq!(innermost_index(&ds, 12), Some(0));
        assert_eq!(innermost_index(&ds, 21), None);
        // Never looked: act. Same declaration: do not. Moving out of one (to none) acts.
        assert!(declaration_changed(None, Some(1)));
        assert!(declaration_changed(None, None));
        assert!(!declaration_changed(Some(Some(1)), Some(1)));
        assert!(declaration_changed(Some(Some(1)), Some(0)));
        assert!(declaration_changed(Some(Some(1)), None));
        assert!(!declaration_changed(Some(None), None));
        // A one-sided declaration follows through the line diff, with no note needed here:
        // keep_place gives the mapped line.
        let hunks = [h(3..3, 3..6)];
        let after = [d("added", 0, 3, 5)];
        assert_eq!(
            keep_place(&after, &[], 4, &hunks, Side::After),
            Placed::Only {
                name: "added".into(),
                line: 3,
                added: true
            }
        );
    }

    #[test]
    fn a_pair_ends_when_a_pane_closes_switches_or_opens_another_file() {
        let link = |b: u64, a: u64| SplitLink {
            before: PaneId(b),
            after: PaneId(a),
            linked: true,
        };
        let links = [link(1, 2), link(3, 4)];
        assert_eq!(links[0].other(PaneId(1)), PaneId(2));
        assert_eq!(links[0].other(PaneId(2)), PaneId(1));
        // Switching sides, or opening another file, in either pane removes that pair only.
        assert_eq!(links_without(&links, PaneId(2)), [link(3, 4)]);
        assert_eq!(links_without(&links, PaneId(3)), [link(1, 2)]);
        assert_eq!(links_without(&links, PaneId(9)), links);
        // Closing either pane removes the pair.
        assert_eq!(
            links_among(&links, &[PaneId(1), PaneId(3), PaneId(4)]),
            [link(3, 4)]
        );
        assert!(links_among(&links, &[]).is_empty());
    }

    #[test]
    fn typed_commits_are_four_to_forty_hex_digits() {
        assert!(looks_like_commit("3f9c"));
        assert!(looks_like_commit(&"a".repeat(40)));
        assert!(!looks_like_commit("3f9"));
        assert!(!looks_like_commit(&"a".repeat(41)));
        assert!(!looks_like_commit("main"));
    }

    #[test]
    fn a_remote_branch_at_its_local_twin_is_dropped() {
        let e = |short: &str, kind, target: &str| RefEntry {
            short: short.into(),
            kind,
            target: target.into(),
        };
        let refs = [
            e("main", RefKind::Branch, "aaaaaaaaaa"),
            e("origin/main", RefKind::RemoteBranch, "aaaaaaaaaa"),
            e("origin/feature", RefKind::RemoteBranch, "bbbbbbbbbb"),
            // Same name but a different commit: kept.
            e("topic", RefKind::Branch, "cccccccccc"),
            e("origin/topic", RefKind::RemoteBranch, "dddddddddd"),
            e("v1", RefKind::Tag, "aaaaaaaaaa"),
        ];
        let none = cho_git::PreferredRefs::default();
        let names: Vec<_> = ref_rows(&refs, &none).into_iter().map(|r| r.0).collect();
        assert_eq!(
            names,
            ["main", "origin/feature", "topic", "origin/topic", "v1"]
        );
    }

    #[test]
    fn the_default_branch_then_the_upstream_come_first() {
        let e = |short: &str, kind, target: &str| RefEntry {
            short: short.into(),
            kind,
            target: target.into(),
        };
        let refs = [
            e("feature", RefKind::Branch, "aaaaaaaaaa"),
            e("main", RefKind::Branch, "bbbbbbbbbb"),
            e("origin/feature", RefKind::RemoteBranch, "cccccccccc"),
            e("origin/main", RefKind::RemoteBranch, "dddddddddd"),
            e("v1", RefKind::Tag, "bbbbbbbbbb"),
        ];
        let p = cho_git::PreferredRefs {
            default_branch: Some("main".into()),
            upstream: Some("origin/feature".into()),
        };
        let rows = ref_rows(&refs, &p);
        let names: Vec<_> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(
            names,
            ["main", "origin/feature", "feature", "origin/main", "v1"]
        );
        assert!(rows[0].1.starts_with("default branch"));
        assert!(rows[1].1.starts_with("upstream"));
        // With only the remote branch, it stands in for the default.
        let remote_only = [
            e("a", RefKind::Branch, "aaaaaaaaaa"),
            e("origin/trunk", RefKind::RemoteBranch, "cccccccccc"),
        ];
        let p = cho_git::PreferredRefs {
            default_branch: Some("trunk".into()),
            upstream: None,
        };
        assert_eq!(ref_rows(&remote_only, &p)[0].0, "origin/trunk");
    }

    #[test]
    fn before_opens_from_git_when_the_working_file_is_too_large() {
        use cho_core::test_repo::git;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "one"]);
        // The working file grows past the reader's limit: it can't be read, but Before (from
        // git) doesn't need it.
        let big = std::fs::File::create(root.join("a.rs")).unwrap();
        big.set_len(cho_core::fs::MAX_SOURCE_BYTES + 1).unwrap();
        let theme = Theme::bundled(cho_theme::Appearance::Dark);
        let path = root.join("a.rs");
        let before = build_other(
            &root,
            "HEAD",
            &path,
            Side::Before,
            "a.rs".into(),
            &theme,
            false,
        )
        .expect("Before comes from git and doesn't need the working file");
        assert_eq!(before.doc.unwrap().buffer.text(), "fn a() {}\n");
        // After still says the working file can't be read.
        assert!(
            build_other(
                &root,
                "HEAD",
                &path,
                Side::After,
                "a.rs".into(),
                &theme,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn the_column_is_kept_clamped_to_the_line_and_a_char_boundary() {
        let text = "ab\nh\u{e9}llo\nxyz";
        assert_eq!(byte_in_line(text, 3..9, 0), 3);
        // Column 2 would split the two-byte `\u{e9}` (bytes 4..6): rounds down.
        assert_eq!(byte_in_line(text, 3..9, 2), 4);
        assert_eq!(byte_in_line(text, 3..9, 3), 6);
        // Past the end of a shorter line: the line end.
        assert_eq!(byte_in_line(text, 0..2, 9), 2);
    }

    #[test]
    fn the_notice_names_the_base_and_several_merge_bases() {
        let full = "3f9c1e2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(
            comparing_notice("main", full, 1),
            "Comparing with main: Before is 3f9c1e2, the merge base"
        );
        assert!(comparing_notice("main", full, 2).ends_with("2 merge bases; using the newest"));
        // A Before that was given (a link's `base`) is not called the merge base.
        assert_eq!(
            comparing_notice("3f9c1e2", full, 0),
            "Comparing with 3f9c1e2: Before is 3f9c1e2, as given"
        );
    }
}
