//! Investigation format version 2 in the viewer, without GPUI: per-step views laid out as
//! focus pages from named arrangements, several highlights in one editor pane, external nodes,
//! phases, author evidence and runtime values (design/investigations-viewer.md, "Format
//! version 2 in the viewer (decided)"; design/layout.md, "Tour focus pages").
//!
//! The tour names panes and an arrangement, never sizes or positions; `cho-layout` lays a
//! page out by its arrangement (`Workspace::reset_page`), so the same tour always gives the
//! same pages. Everything here is what Chorong decides; what the file claims stays labelled as a claim
//! (evidence and runtime values are the author's, never verification).
//!
//! The format's own v2 types live in `cho-investigation`; [`StepExtras`] is what the viewer
//! needs of them per step, resolved from the check report (`investigation::v2_of`), so the
//! panes can be drawn and tested from fixtures. Positions are UTF-8 byte offsets.

use std::ops::Range;

pub use cho_layout::PageArrangement as Arrangement;

pub use cho_investigation::{Evidence, Phase};

/// One highlighted chunk of an editor pane, resolved: its byte range (whole lines are tinted)
/// and the author's optional note, shown in a strip above its first line.
/// An action step: what the reader is asked to do, with commands to copy. Chorong never runs
/// them (investigation files are untrusted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionCard {
    pub name: String,
    /// Markdown, shown inert.
    pub note: String,
    pub commands: Vec<CommandLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    pub label: Option<String>,
    /// Exactly what Copy puts on the clipboard.
    pub text: String,
}

impl CommandLine {
    /// What Copy puts on the clipboard: the command exactly as written, byte for byte (no
    /// quoting, trimming or reformatting).
    pub fn clipboard(&self) -> &str {
        &self.text
    }

    /// The lines the card shows, one row each: the text's own lines, empty ones kept.
    pub fn display_lines(&self) -> Vec<&str> {
        self.text.split('\n').collect()
    }
}

/// A diff pane: one file between two revisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffAt {
    pub path: String,
    /// As written (`main`).
    pub base: String,
    /// As written; `None` for the working tree.
    pub head: Option<String>,
}

impl DiffAt {
    /// `main → the working tree`.
    pub fn sides(&self) -> String {
        format!(
            "{} \u{2192} {}",
            self.base,
            self.head.as_deref().unwrap_or("the working tree")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// What the anchor pins: its excerpt or `from`..`to` range, else its declaration.
    pub range: Range<usize>,
    /// The declaration the anchor names, when it names one.
    pub region: Option<Range<usize>>,
    /// The short label: the strip above the chunk and its row in the step's list.
    pub label: Option<String>,
    /// Commentary under the label in the step's list.
    pub note: Option<String>,
}

impl Highlight {
    /// What is tinted faintly: the declaration, else the pinned range.
    pub fn area(&self) -> Range<usize> {
        match &self.region {
            Some(r) if r.start <= self.range.start && self.range.end <= r.end => r.clone(),
            _ => self.range.clone(),
        }
    }

    /// The darker part inside [`Highlight::area`]: an excerpt or `from`..`to` range, or `None`
    /// when the chunk is a whole declaration.
    pub fn pinned(&self) -> Option<Range<usize>> {
        (self.area() != self.range).then(|| self.range.clone())
    }
}

/// A pane a step's view asks for, resolved against the code. A broken anchor is a
/// [`PaneSpec::Broken`] placeholder that says why; nothing is put in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneSpec {
    /// A file at its anchor, with its highlights (none: the anchor's own region is tinted).
    Editor {
        path: String,
        caret: Option<usize>,
        region: Option<Range<usize>>,
        highlights: Vec<Highlight>,
        /// What the anchor asked for, for the header: `at Queue::push`.
        at: String,
    },
    /// Context: for a symbol the view names, a pin of it (at `path` and byte `at`); without
    /// one, the live Context pane.
    Context {
        symbol: Option<String>,
        path: Option<String>,
        at: Option<usize>,
    },
    Outline {
        path: String,
    },
    /// A graph `of` a node or anchor. Drawn as a placeholder until the graph pane lands.
    /// A graph around `of` (`in`, `out` or `both`), drawn by the graph pane from the index,
    /// starting at the declaration at `path` and byte `at`.
    Graph {
        of: String,
        direction: String,
        depth: u32,
        path: Option<String>,
        at: Option<usize>,
    },
    CallStack,
    FileTree,
    /// How one file changed: an editor with the comparison view's change marks.
    Diff(DiffAt),
    /// A diff pane that can't be shown (an unresolved revision): a card saying why.
    DiffUnresolved {
        diff: DiffAt,
        reason: String,
    },
    /// An action step's card.
    Action(std::sync::Arc<ActionCard>),
    /// A node that is not in the code: a small card instead of an editor.
    External {
        title: String,
        note: Option<String>,
    },
    /// An anchor in the view that did not resolve.
    Broken {
        asked: String,
        reason: String,
    },
    /// A comparison side with no step for the row: why (the author's words).
    Missing {
        side: String,
        reason: String,
    },
}

impl PaneSpec {
    /// The placeholder pane's name key for panes the viewer draws itself (broken, external,
    /// graph until the graph pane lands); `None` for real panes.
    pub fn card_title(&self) -> Option<String> {
        match self {
            PaneSpec::Broken { asked, .. } => Some(format!("Anchor not found: {asked}")),
            PaneSpec::External { title, .. } => Some(format!("{title} (not in the code)")),
            PaneSpec::Action(a) => Some(format!("Action: {}", a.name)),
            PaneSpec::DiffUnresolved { diff, .. } => Some(format!("Diff not shown: {}", diff.path)),
            PaneSpec::Missing { side, .. } => Some(format!("{side}: missing")),
            PaneSpec::Graph { of, direction, .. } => {
                Some(format!("Graph: {of} \u{b7} {direction}"))
            }
            _ => None,
        }
    }
}

/// The names of the card panes (broken anchors, external nodes, graphs) of a step's page,
/// one per pane (`None` for real panes): unique within the page, and across pages by the
/// step's number.
pub fn card_names(view: &View, number: &str) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = Vec::new();
    for spec in &view.panes {
        out.push(spec.card_title().map(|base| {
            let name = format!("{base} \u{b7} step {number}");
            let dupes = out
                .iter()
                .flatten()
                .filter(|n| n.starts_with(&name))
                .count();
            if dupes == 0 {
                name
            } else {
                format!("{name} ({})", dupes + 1)
            }
        }));
    }
    out
}

/// What a step's view gives its page, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub panes: Vec<PaneSpec>,
    pub arrangement: Arrangement,
}

/// What format version 2 adds to a step, as the viewer needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepExtras {
    /// A short name for lists; the note stays in the step note.
    pub name: Option<String>,
    pub phase: Option<Phase>,
    pub evidence: Option<Evidence>,
    /// Runtime values the author saw (`flag = false`, "refused"): always the author's claim.
    pub runtime: Vec<String>,
    /// Set for a node that is not in the code (a model, a database, a vendor): what it is.
    pub external: Option<String>,
    /// An action step: an instruction, not code.
    pub action: Option<std::sync::Arc<ActionCard>>,
    /// The step's view, when it declares one.
    pub view: Option<View>,
    /// Set when the chosen tour branch does not reach this step: why (the author's words).
    /// The step stays in the list, muted, and stepping skips it.
    pub not_reached: Option<String>,
    /// A comparison row shown as a step: its result and each side's cell, in side order.
    pub row: Option<std::sync::Arc<crate::comparison::RowInfo>>,
}

/// What format version 2 adds to the whole investigation, for the pane headers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overview {
    pub bullets: Vec<String>,
    /// Calls on the path that could not be traced, each with the author's words.
    pub untraced: Vec<String>,
}

/// An action step's page: its card as the main pane, then the panes its view asks for.
pub fn action_view(card: std::sync::Arc<ActionCard>, view: Option<&View>) -> View {
    let mut panes = vec![PaneSpec::Action(card)];
    let mut arrangement = Arrangement::Single;
    if let Some(v) = view {
        panes.extend(v.panes.iter().cloned());
        arrangement = if v.arrangement == Arrangement::Single {
            Arrangement::MainSide
        } else {
            v.arrangement
        };
    }
    View { panes, arrangement }
}

/// The page a step without a view gets: the editor at its anchor, plus Context. An external
/// node gets its card in place of the editor.
pub fn default_view(
    path: &str,
    caret: Option<usize>,
    region: Option<Range<usize>>,
    at: &str,
    external: Option<(&str, Option<&str>)>,
    broken: Option<&str>,
) -> View {
    let first = match (external, broken) {
        (Some((title, note)), _) => PaneSpec::External {
            title: title.to_string(),
            note: note.map(str::to_string),
        },
        (None, Some(reason)) => PaneSpec::Broken {
            asked: if at.is_empty() {
                path.to_string()
            } else {
                format!("{at} in {path}")
            },
            reason: reason.to_string(),
        },
        (None, None) => PaneSpec::Editor {
            path: path.to_string(),
            caret,
            region,
            highlights: Vec::new(),
            at: at.to_string(),
        },
    };
    View {
        panes: vec![
            first,
            PaneSpec::Context {
                symbol: None,
                path: None,
                at: None,
            },
        ],
        arrangement: Arrangement::MainSide,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_highlight_tints_its_declaration_and_pins_its_excerpt() {
        let h = |range, region| Highlight {
            range,
            region,
            label: None,
            note: None,
        };
        // An excerpt inside a declaration: the declaration faint, the excerpt darker.
        let excerpt = h(40..50, Some(10..90));
        assert_eq!((excerpt.area(), excerpt.pinned()), (10..90, Some(40..50)));
        // A whole declaration: nothing darker.
        let whole = h(10..90, Some(10..90));
        assert_eq!((whole.area(), whole.pinned()), (10..90, None));
        // No declaration (a line or a file): the range itself.
        let line = h(5..9, None);
        assert_eq!((line.area(), line.pinned()), (5..9, None));
        // A region that does not hold the range is not used.
        let odd = h(95..99, Some(10..90));
        assert_eq!((odd.area(), odd.pinned()), (95..99, None));
    }

    #[test]
    fn a_step_without_a_view_gets_the_editor_and_context() {
        let v = default_view("src/a.rs", Some(4), Some(0..9), "Queue::push", None, None);
        assert_eq!(v.arrangement, Arrangement::MainSide);
        assert!(matches!(&v.panes[0], PaneSpec::Editor { path, .. } if path == "src/a.rs"));
        assert!(matches!(v.panes[1], PaneSpec::Context { symbol: None, .. }));
        let broken = default_view("src/w.rs", None, None, "Worker::drain", None, Some("gone"));
        assert_eq!(
            broken.panes[0],
            PaneSpec::Broken {
                asked: "Worker::drain in src/w.rs".into(),
                reason: "gone".into()
            }
        );
        let ext = default_view("", None, None, "", Some(("Postgres", None)), None);
        assert!(matches!(&ext.panes[0], PaneSpec::External { title, .. } if title == "Postgres"));
    }

    #[test]
    fn copy_gives_the_command_byte_for_byte_and_the_card_keeps_its_lines() {
        let text =
            "psql -c '\\d orders' \\\n  && echo \"done; $HOME\"\n\n\tcargo test -- --nocapture  ";
        let cmd = CommandLine {
            label: None,
            text: text.to_string(),
        };
        assert_eq!(cmd.clipboard().as_bytes(), text.as_bytes());
        assert_eq!(
            cmd.display_lines(),
            [
                "psql -c '\\d orders' \\",
                "  && echo \"done; $HOME\"",
                "",
                "\tcargo test -- --nocapture  "
            ]
        );
    }

    #[test]
    fn an_action_page_is_its_card_then_the_panes_it_asks_for() {
        let card = std::sync::Arc::new(ActionCard {
            name: "Run it".into(),
            note: "n".into(),
            commands: vec![],
        });
        let alone = action_view(card.clone(), None);
        assert_eq!(alone.arrangement, Arrangement::Single);
        assert_eq!(alone.panes, vec![PaneSpec::Action(card.clone())]);
        let editor = PaneSpec::Editor {
            path: "src/a.rs".into(),
            caret: None,
            region: None,
            highlights: vec![],
            at: "a".into(),
        };
        let one = View {
            panes: vec![editor.clone()],
            arrangement: Arrangement::Single,
        };
        let v = action_view(card.clone(), Some(&one));
        assert_eq!(v.arrangement, Arrangement::MainSide, "two panes now");
        assert_eq!(v.panes[1], editor);
        let named = View {
            arrangement: Arrangement::SideBySide,
            ..one
        };
        assert_eq!(
            action_view(card.clone(), Some(&named)).arrangement,
            Arrangement::SideBySide
        );
        assert_eq!(
            card_names(&v, "2")[0].as_deref(),
            Some("Action: Run it \u{b7} step 2")
        );
    }

    #[test]
    fn card_panes_get_names_unique_on_the_page() {
        let broken = |a: &str| PaneSpec::Broken {
            asked: a.into(),
            reason: "gone".into(),
        };
        let v = View {
            panes: vec![
                broken("x"),
                PaneSpec::Context {
                    symbol: None,
                    path: None,
                    at: None,
                },
                broken("x"),
            ],
            arrangement: Arrangement::Grid,
        };
        let names = card_names(&v, "S2");
        assert_eq!(
            names[0].as_deref(),
            Some("Anchor not found: x \u{b7} step S2")
        );
        assert_eq!(names[1], None);
        assert_eq!(
            names[2].as_deref(),
            Some("Anchor not found: x \u{b7} step S2 (2)")
        );
    }
}
