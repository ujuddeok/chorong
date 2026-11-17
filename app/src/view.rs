//! The main window: file sidebar, source view, context and outline panels, file picker,
//! status bar.
//!
//! Heavy work (reading, highlighting, outlining, loading and querying the index) runs on
//! GPUI's background executor; generation counters stop a slow, older result from replacing
//! a newer one. The source list renders `Folds::rows`, a visible-row mapping over the
//! untouched buffer. The caret is a UTF-8 byte offset; the selected line follows it, and the
//! context pane follows the caret without being asked, or the identifier under the pointer
//! once it rests there. Repository search runs in `cho-search` on the background executor,
//! debounced, with a generation guard and a cancel token.
//!
//! This file holds the window's state, construction, key handling and top-level layout. Each
//! pane lives in its own module, named after `cho_layout::PaneKind` where it can be: `file_tree`,
//! `outline`, `editor`, `context_pane` (also the pinned pane), `search_pane` and `graph_ui`.
//! The modules shared by all panes are `navigation` (opening and moving), `focus` (what the
//! context pane follows), `picker_ui`, `index_status` (the index and status bar), `overlays`,
//! `mismatch` and `commands`.

mod action_card;
mod blame;
mod callstack_pane;
mod change_marks;
mod changes;
mod commands;
mod compare;
mod compare_git;
mod comparison_pane;
mod context_pane;
mod diff_pane;
mod editor;
mod editor_state;
mod file_tree;
mod find_bar;
mod focus;
mod graph_pane;
mod graph_ui;
mod highlights;
mod index_status;
mod investigation_view;
mod link_view;
mod mismatch;
mod navigation;
mod outline;
mod overlays;
mod peek;
mod picker_ui;
mod pins;
mod prepare;
mod remote;
mod row_list;
mod search_pane;
mod settings_apply;
mod settings_cmds;
mod shell_host;
mod text_select;
mod tour_pages_view;
mod tour_pane;

use crate::Startup;
use crate::actions::*;
use crate::callkind::{self, CallKind, ProjectPackages};
use crate::context::{self, Answer, Place};
use crate::document::{Document, hsla};
use crate::features::{Feature, Features};
use crate::files::list_files;
use crate::fold::{Folds, Row};
use crate::generation::Generation;
use crate::history::Entry;
use crate::hover::Hover;
use crate::indexing;
use crate::keymap::{self, Platform};
use crate::lens::{self, KindedCall, lens_highlights};
use crate::links::{self, LinkInfo};
use crate::markers;
use crate::marks::{Marks, Source, enclosing, identifier_at};
use crate::picker::{MAX_RESULTS, Match, Ranker};
use crate::pointer::ClickKind;
use crate::search::{Row as SearchRow, SearchState};
use crate::trust::TrustStore;
use cho_core::{Editor, EditorLocation, Launch};
use cho_layout::PaneId;
use cho_layout_ui::field::{FieldEdit, edit_field};
use cho_layout_ui::shell::{Shell, ShellOptions, bind_actions};
use cho_search::{CancelToken, SearchOptions, search_all};
use cho_semantic::{IndexCache, IndexStatus, Semantic, cache::overall_status};
use cho_syntax::{SymbolMatch, find_symbol};
use cho_theme::{Rgba, Theme};
use compare::{Comparison, Gutter, Side};
/// The code view's copy actions, for the macOS Edit menu (`main.rs`).
pub(crate) use editor::{CopySelection, CopyWithLocation, SelectAll};
use editor_state::EditorState;
use gpui::{
    Action, AnyView, App, Context, Div, Entity, FocusHandle, FontWeight, HighlightStyle,
    KeyContext, KeyDownEvent, Keystroke, ListHorizontalSizingBehavior, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, ScrollStrategy, SharedString, StyledText, Subscription,
    UniformListScrollHandle, Window, actions, div, prelude::*, px,
};
/// Settings…, for the macOS app menu (`main.rs`).
pub(crate) use settings_cmds::OpenSettings;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const LINE_HEIGHT: f32 = 20.;
const FONT_SIZE: f32 = 13.;
const GUTTER_WIDTH: f32 = 56.;
/// Space between a line number (or the fold chevron in its place) and the code.
const NUMBER_PAD: f32 = 12.;
const PICKER_WIDTH: f32 = 600.;
/// The preview column beside Go to file… (`picker.preview`).
const PEEK_WIDTH: f32 = 560.;
const PICKER_ROW: f32 = 30.;
/// Space around the result rows, inside the picker.
const PICKER_INSET: f32 = 6.;
const PICKER_VISIBLE_ROWS: usize = 12;
/// Wait after the last keystroke before a search starts.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
/// How long a status-bar notice stays.
const NOTICE_TIME: Duration = Duration::from_secs(4);
/// How long "Cancel indexing?" waits for the second click.
const CANCEL_CONFIRM: Duration = Duration::from_secs(4);
/// A second Esc within this long clears every highlight group.
const ESC_AGAIN: Duration = Duration::from_millis(2500);
/// After the pointer leaves the source or the context pane, how long it may take to reach the
/// other before the pane goes back to the caret.
const LEAVE_GRACE: Duration = Duration::from_millis(150);
/// A chord dropped at least this long after it started was dropped by GPUI's timeout (one
/// second, fixed in GPUI), not by a key.
const CHORD_HOLD_AFTER: Duration = Duration::from_millis(500);

/// Where to land in a newly shown file.
#[derive(Default, Clone)]
pub struct Target {
    /// Exact caret position; wins over `symbol` and `line`.
    pub byte: Option<usize>,
    pub symbol: Option<String>,
    /// Zero-based line.
    pub line: Option<usize>,
    /// The target of a `chorong://` link: its symbol lookup feeds the commit-mismatch banner,
    /// and its line is not used at another commit.
    pub from_link: bool,
    /// A declaration or line on one side of a comparison (from the Changes list).
    pub place: Option<changes::ChangePlace>,
    /// A tour or call-stack step: no landing outline, since the step's tint shows what it is
    /// about and the landing would mark only wherever the caret fell.
    pub step: bool,
}

impl Target {
    /// A symbol and a one-based line (as links and the command line give them).
    pub fn at(symbol: Option<String>, line_1based: Option<usize>) -> Self {
        Target {
            symbol,
            line: line_1based.map(|l| l.saturating_sub(1)),
            ..Target::default()
        }
    }
}

/// What is known about the semantic index of the opened folder.
enum IndexState {
    /// No folder, or loading has not started.
    Absent,
    Loading,
    /// No usable index: `none`, or `failed` when the last run failed and left nothing.
    Missing(IndexStatus),
    Ready(Arc<Semantic>, IndexStatus),
    /// The cache exists but could not be read.
    Failed(String),
}

impl IndexState {
    fn status_text(&self) -> String {
        match self {
            IndexState::Absent => "none".into(),
            IndexState::Loading => "loading".into(),
            IndexState::Ready(_, IndexStatus::Partial) => "partial (prepare failed)".into(),
            IndexState::Missing(s) | IndexState::Ready(_, s) => s.as_str().into(),
            IndexState::Failed(_) => "failed".into(),
        }
    }
}

/// What loading the cached index found.
struct Loaded {
    state: IndexState,
    packages: ProjectPackages,
    /// The stored stderr tail of a project whose last build failed.
    failure: Option<String>,
}

/// Loads the cached index for `root`. Runs on a background thread.
fn load_index(root: &std::path::Path) -> Loaded {
    let packages = ProjectPackages::default();
    let Some(cache) = IndexCache::default_location() else {
        return Loaded {
            state: IndexState::Missing(IndexStatus::None),
            packages,
            failure: None,
        };
    };
    let failure = root.canonicalize().ok().and_then(|r| {
        cache
            .projects(&r)
            .into_iter()
            .find(|p| !p.meta.success())
            .map(|p| {
                let name = p.dir.file_name().map(|n| n.to_string_lossy().into_owned());
                format!(
                    "{} ({}) failed{}:\n{}",
                    name.unwrap_or_default(),
                    p.meta.indexer,
                    if p.meta.timed_out() {
                        ", timed out"
                    } else {
                        ""
                    },
                    p.meta.stderr_tail.trim_end()
                )
            })
    });
    match Semantic::open(&cache, root) {
        Ok(Some(sem)) => {
            let status = sem.status();
            let packages = ProjectPackages::from_index(sem.index());
            Loaded {
                state: IndexState::Ready(Arc::new(sem), status),
                packages,
                failure,
            }
        }
        Ok(None) => {
            let status = root
                .canonicalize()
                .map(|r| overall_status(&cache.projects(&r)))
                .unwrap_or(IndexStatus::None);
            Loaded {
                state: IndexState::Missing(status),
                packages,
                failure,
            }
        }
        Err(e) => Loaded {
            state: IndexState::Failed(e.to_string()),
            packages,
            failure,
        },
    }
}

/// An index lookup for one place: not asked (no index, or nothing there), running, or
/// answered.
#[derive(Default)]
enum Lookup {
    #[default]
    Idle,
    Loading,
    Done(Answer),
}

impl Lookup {
    /// The symbol the lookup found, once answered.
    fn symbol(&self) -> Option<&context::SymbolContext> {
        match self {
            Lookup::Done(Answer::Symbol(c)) => Some(c),
            _ => None,
        }
    }
}

/// The context pane's index answer for the identifier under the caret.
#[derive(Default)]
struct ContextState {
    /// The (file, identifier start) the answer is for.
    key: Option<(PathBuf, usize)>,
    lookup: Lookup,
    generation: Generation,
}

/// The top of the context pane: where the focus is, from Tree-sitter.
#[derive(Clone)]
struct PaneFocus {
    /// The enclosing declaration: kind, qualified name, first and last line (one-based).
    decl: Option<(String, String, usize, usize)>,
    /// The identifier at the focus.
    ident: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickerKind {
    Compare,
    Files,
    Palette,
    Recent,
    Symbols,
    /// Open Investigation…: the repository's investigations, then recents.
    Investigations,
}

impl PickerKind {
    fn placeholder(self) -> &'static str {
        match self {
            PickerKind::Compare => "Compare against a branch, tag or commit\u{2026}",
            PickerKind::Files if cfg!(target_os = "macos") => {
                "Go to file\u{2026}  \u{2318}\u{21a9} right  \u{2318}\u{21e7}\u{21a9} below  \u{2325}\u{21a9} tab"
            }
            // The keys are in the footer.
            PickerKind::Files => "Go to file\u{2026}",
            PickerKind::Palette => "Type a command\u{2026}",
            PickerKind::Recent => "Open a folder opened before\u{2026}",
            PickerKind::Symbols => "Go to symbol in this file\u{2026}",
            PickerKind::Investigations => "Open an investigation\u{2026}",
        }
    }
}

impl PickerState {
    fn placeholder(&self) -> &'static str {
        match self.add_to_page {
            true if cfg!(target_os = "macos") => {
                "Add a file to this page\u{2026}  \u{21a9} right  \u{21e7}\u{21a9} below  \u{232b} tray"
            }
            true => {
                "Add a file to this page\u{2026}  (Enter right, Shift-Enter below, Backspace tray)"
            }
            false => self.kind.placeholder(),
        }
    }
}

/// What confirming a picker row does (non-file pickers).
enum Pick {
    Command(Box<dyn Action>),
    /// A ref (or typed commit) to compare against: what the row shows, and the revision.
    Ref {
        label: String,
        rev: String,
    },
    Symbol(usize),
    /// A folder opened before.
    Folder(PathBuf),
    /// An investigation to open.
    Investigation(crate::investigation::Source),
    /// The Open dialog for any investigation file.
    InvestigationFile,
    /// A row that cannot be opened (a missing recent, a file that does not parse): why.
    Unavailable(String),
}

struct PickerState {
    kind: PickerKind,
    query: String,
    /// The whole query is selected (Cmd-A).
    query_all: bool,
    /// The file picker adds each file to the current focus page and stays open
    /// (`AddFileToPage`, from Bring from tray… or New Focus Page).
    add_to_page: bool,
    /// Go to file… while a tour is open: only the tour's files (the Tour chip before the
    /// query). Backspace on an empty query takes it off; Alt-T puts it back.
    tour_scope: bool,
    /// Go to file…'s preview beside the list is open (→ opens it, ← closes it).
    peek_open: bool,
    matches: Vec<Match>,
    selected: usize,
    /// Candidates for the palette and the symbol picker (files use `Chorong::file_names`).
    labels: Vec<String>,
    /// Right-aligned text per candidate: a key binding or a declaration kind.
    hints: Vec<String>,
    picks: Vec<Pick>,
}

/// A full-window layer over the code view.
enum Overlay {
    /// The `?` cheat sheet.
    Cheat,
    /// The once-per-repository question before the first index build.
    Trust { notes: Vec<String> },
    /// A `chorong://` link names a folder Chorong has never opened: ask before opening it.
    OpenLink { url: String, folder: PathBuf },
    /// A pull-request link and nothing cached for it: ask before fetching the pull request.
    OpenPullRequest(crate::AskPr),
    /// The stderr tail of a failed index build.
    IndexError,
    /// Approval to prepare and index a remote project's worktree.
    PrepareApproval(crate::remote::prepare::Panel),
    /// The Show graph popover.
    Graph(graph_ui::GraphPopover),
}

/// The Cmd-K chord: GPUI drops a chord after one second (fixed in GPUI); Chorong holds it open,
/// popup and all, until the next key when no key was pressed.
#[derive(Default)]
enum Chord {
    #[default]
    Idle,
    /// In progress since this moment, to tell GPUI's timeout from a chord that was answered.
    Pending(Vec<Keystroke>, Instant),
    /// Timed out while its popup was showing: held open until the next key.
    Held(Vec<Keystroke>),
}

/// Which list or field has the keyboard when no overlay, form, picker or held chord takes it.
/// One value, so two lists never both have it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum KeyFocus {
    /// The code view, the only place with single-letter keys.
    #[default]
    Code,
    /// The search panel's query.
    Search,
    /// The context pane's reference list (Shift-F12, `r`), on this row, counting definitions
    /// first.
    References { row: usize },
    /// The Changes list.
    Changes,
    /// The Tour or Call stack pane's steps.
    Steps,
    /// The active code pane's find bar (Cmd-F).
    Find,
}

impl KeyFocus {
    fn key_context(self) -> &'static str {
        match self {
            KeyFocus::Code => "Chorong CodeView",
            KeyFocus::Search => "Chorong Search",
            KeyFocus::References { .. } => "Chorong References",
            KeyFocus::Changes => "Chorong Changes",
            KeyFocus::Steps => "Chorong Steps",
            KeyFocus::Find => find_bar::CONTEXT,
        }
    }

    /// `from` (any row, for References) closed or let go: the keyboard goes back to the code
    /// if `from` had it, and stays where it is otherwise.
    fn leave(&mut self, from: KeyFocus) {
        if std::mem::discriminant(self) == std::mem::discriminant(&from) {
            *self = KeyFocus::Code;
        }
    }

    /// The selected reference row, while the reference list has the keyboard.
    fn reference_row(self) -> Option<usize> {
        match self {
            KeyFocus::References { row } => Some(row),
            _ => None,
        }
    }
}

/// How an overlay takes the keyboard: its key context, and the keys it shows in its buttons
/// and headings.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OverlayKind {
    /// A question with Accept and Decline.
    Dialog,
    Graph,
    /// A sheet to read and close (cheat sheet, index error).
    Sheet,
}

impl OverlayKind {
    fn key_context(self) -> &'static str {
        match self {
            OverlayKind::Dialog => "Chorong Dialog",
            OverlayKind::Graph => "Chorong Graph",
            OverlayKind::Sheet => "Chorong Sheet",
        }
    }

    /// The actions whose keys the overlay shows, as bound in its own context.
    fn shown_actions(self) -> &'static [&'static str] {
        match self {
            OverlayKind::Dialog => &["chorong::TrustAccept", "chorong::TrustDecline"],
            OverlayKind::Graph => &[
                "chorong::GraphDirection",
                "chorong::GraphDeeper",
                "chorong::GraphShare",
                "chorong::GraphBuild",
                "chorong::GraphClose",
            ],
            OverlayKind::Sheet => &[],
        }
    }
}

impl Overlay {
    fn kind(&self) -> OverlayKind {
        match self {
            Overlay::Trust { .. }
            | Overlay::OpenLink { .. }
            | Overlay::OpenPullRequest(_)
            | Overlay::PrepareApproval(_) => OverlayKind::Dialog,
            Overlay::Graph(_) => OverlayKind::Graph,
            Overlay::Cheat | Overlay::IndexError => OverlayKind::Sheet,
        }
    }
}

/// An index build in progress.
struct Job {
    total: usize,
    done: usize,
    current: String,
    /// Trusted open or stale refresh, as opposed to the reader asking.
    automatic: bool,
    /// Stops the indexer subprocess (and its children) when set.
    cancel: cho_semantic::indexer::CancelToken,
    cancel_state: Cancel,
    started: Instant,
    /// The indexer's step as of the last poll (every two seconds).
    progress: cho_semantic::progress::Progress,
    /// Written by the build thread, read by the poll.
    live: Arc<std::sync::Mutex<cho_semantic::progress::Progress>>,
    /// A large workspace: the up-front note about time and memory is shown.
    large: bool,
}

/// Where cancelling a build stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cancel {
    /// Not asked.
    No,
    /// The first click on the status-bar item asked "Cancel indexing?"; a second one before
    /// this moment cancels.
    Confirming(Instant),
    /// Cancel was asked for; the subprocess is being stopped.
    Cancelling,
}

/// A brief message in the status bar ("not available yet", build results).
struct Notice {
    text: String,
    generation: Generation,
}

/// A strip above the source with a message about the link that was opened and what to do
/// about it ("not on this machine", "made at another commit").
pub(crate) struct Banner {
    text: String,
    /// A second line: details, or a command to copy.
    detail: Option<String>,
    actions: Vec<(String, BannerAction)>,
    /// A smaller note rather than a warning.
    quiet: bool,
}

#[derive(Clone)]
pub(crate) enum BannerAction {
    OpenUrl(String),
    Copy(String),
    ViewAtRev,
    BackToCheckout,
    OpenWorktree(PathBuf),
    RebuildGraph,
    Dismiss,
}

/// A plain-text tooltip.
struct TextTip(SharedString);

impl Render for TextTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        tip_box().child(self.0.clone())
    }
}

/// How a list scrolls to keep a row stepped onto by `delta` in view: down keeps it at the
/// bottom edge, up at the top.
fn step_strategy(delta: isize) -> ScrollStrategy {
    if delta > 0 {
        ScrollStrategy::Bottom
    } else {
        ScrollStrategy::Top
    }
}

/// The box every tooltip is drawn in: dark, bordered, 12 px text.
fn tip_box() -> Div {
    div()
        .px_2()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(gpui::hsla(0., 0., 0.5, 0.4))
        .bg(gpui::hsla(0., 0., 0.1, 0.95))
        .text_color(gpui::hsla(0., 0., 0.95, 1.))
        .text_size(px(12.))
}

/// A tooltip with a note's styled text ([`note_tip`]); looks like [`TextTip`].
struct NoteTip(crate::note_md::Text);

impl Render for NoteTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        tip_box().max_w(px(520.)).child(note_text(
            &self.0,
            gpui::hsla(0.58, 0.8, 0.75, 1.),
            gpui::hsla(0., 0., 0.7, 1.),
            gpui::hsla(0., 0., 1., 0.12),
        ))
    }
}

/// The banner for a link to a repository with no worktree on this machine.
fn elsewhere_banner(info: &LinkInfo, web: Option<String>) -> Banner {
    let mut actions = Vec::new();
    if let Some(url) = &web {
        actions.push((
            "Open on the web".to_string(),
            BannerAction::OpenUrl(url.clone()),
        ));
    }
    actions.push(("Dismiss".to_string(), BannerAction::Dismiss));
    Banner {
        text: format!(
            "{} is not open in Chorong on this machine. Open a clone of it once, and links to it will open here.",
            info.repo
        ),
        detail: web,
        actions,
        quiet: false,
    }
}

fn tip(text: String) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| cx.new(|_| TextTip(text.clone().into())).into()
}

/// A tooltip with an investigation note's text: emphasis, code spans and links styled as in
/// [`note_text`].
fn note_tip(text: crate::note_md::Text) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| cx.new(|_| NoteTip(text.clone())).into()
}

/// An investigation note (or name, or callout) as one text element: bold and italic as
/// written, code spans on a code tint, links in the link colour with their real target shown
/// (never opened from here). For surfaces that are one line or one wrapping run of text.
fn note_text(
    text: &crate::note_md::Text,
    link: gpui::Hsla,
    inert: gpui::Hsla,
    code_bg: gpui::Hsla,
) -> StyledText {
    use crate::investigation::LinkKind;
    StyledText::new(text.text.clone()).with_highlights(text.runs.iter().map(|(r, l)| {
        let mut st = HighlightStyle {
            font_weight: l.bold.then_some(FontWeight::BOLD),
            font_style: l.italic.then_some(gpui::FontStyle::Italic),
            background_color: l.code.then_some(code_bg),
            ..HighlightStyle::default()
        };
        match l.link {
            Some(LinkKind::Inert) => st.color = Some(inert),
            Some(_) => {
                st.color = Some(link);
                st.underline = Some(gpui::UnderlineStyle {
                    thickness: px(1.),
                    color: Some(link),
                    wavy: false,
                });
            }
            None => {}
        }
        (r.clone(), st)
    }))
}

/// [`note_text`] in the pane colours.
fn note_text_in(text: &crate::note_md::Text, c: &cho_layout_ui::colors::Colors) -> StyledText {
    note_text(text, c.text_accent, c.text_muted, c.hover)
}

pub struct Chorong {
    theme: Theme,
    editor: Editor,
    font: SharedString,
    /// The code font given with `--font`, which overrides the `appearance.code_font` setting.
    cli_font: Option<SharedString>,
    root: Option<PathBuf>,
    files: Vec<PathBuf>,
    /// Show graph: the popover's remembered choices, the reference graph built from the
    /// loaded index (kept until the index changes), whether a build is running, and the
    /// popover's key labels from the live keymap.
    graph_opts: crate::graph::GraphOptions,
    ref_graph: Option<(Arc<Semantic>, Arc<cho_graph::RefGraph>)>,
    graph_building: bool,
    /// Graph panes (`graph_pane.rs`): each one's spec, layout and zoom, by key.
    graph_panes: graph_pane::GraphPanes,
    overlay_keys: HashMap<&'static str, String>,
    /// The find bars' shared state: the last query and the bar's key labels.
    find: find_bar::FindShared,
    /// Shift tapped twice opens the command palette.
    shift_taps: crate::double_tap::ShiftTaps,
    /// `files` as display strings, built once for the picker.
    file_names: Vec<String>,
    /// The file preview of Go to file… and full-screen search (`peek.rs`).
    peek: peek::PeekState,
    /// Search was grown to fill the window (Cmd-Shift-F again): the focus layout it opened.
    search_full: bool,
    /// The step banner's height when the reader has dragged it, for every pane in the window;
    /// `None` fits it to its text up to a cap.
    note_height: Option<f32>,
    fold_by_default: bool,
    /// Collapsed-fold state of files shown earlier this session.
    fold_memory: HashMap<PathBuf, HashSet<usize>>,
    lens: bool,
    /// Toggle Blame is on: editor panes show the blame column (while `blame.enabled` is).
    /// Blame answers by folder, file, commit and text, shared with background jobs.
    blame_cache: Arc<std::sync::Mutex<crate::blame::Cache>>,
    /// The Before/After comparison this window is showing, if any.
    compare: Option<Comparison>,
    /// How many files differ between HEAD and the working folder, while comparing (`None`
    /// until counted), and the count in flight, to drop a stale answer.
    uncommitted: Option<usize>,
    uncommitted_gen: Generation,
    /// Side-by-side pairs of one file (Before left, After right), and whether a caret move is
    /// being followed (so the follow itself does not follow).
    links: Vec<compare::SplitLink>,
    following: bool,
    /// The folder is in a git repository with a working folder: the status bar offers
    /// Compare… there. Found once, when the window opens.
    git_folder: bool,
    /// Highlight Uncommitted (Cmd-K U) is on: uncommitted lines are tinted yellow.
    highlight_uncommitted: bool,
    /// The side the window shows while comparing: `b` flips every code pane not in a pair to
    /// it, and a newly opened file opens on it.
    window_side: Side,
    /// A link's side and place, applied once the first file and its other side are ready.
    pending_link: Option<link_view::PendingLink>,
    /// One editor state per Editor pane of the layout, and the pane that most recently had
    /// focus: the one the context pane, outline, status bar, commands and opened files use.
    editors: std::collections::BTreeMap<PaneId, EditorState>,
    active_editor: PaneId,
    /// The focused pane the last time the shell changed, to tell a focus change from other
    /// layout changes.
    last_focused: Option<PaneId>,
    /// Stand-in when no editor pane exists (the layout model always has one; never shown).
    fallback_editor: EditorState,
    /// The editor pane the pointer is over (a hover is only followed in the active one).
    hover_pane: PaneId,
    /// A command is running on a pane that is not the active editor (an async load landing).
    pane_override: bool,
    /// Set by a change that affects only this editor pane; read once by the next refresh.
    dirty_pane: Option<PaneId>,
    /// The active editor changed: every pane (the file tree marks the open file) redraws.
    dirty_all: bool,
    /// A pending write of the editor carets into the saved layout (debounced).
    caret_save: Option<gpui::Task<()>>,
    index: IndexState,
    /// Index loads started (each one's generation; a load that lands after a newer one
    /// started is dropped), and the load that set `index`: its identity for the pin panes.
    index_loads: Generation,
    index_id: Generation,
    /// Pin gathers started: a gather lands only on the view it was started for.
    pin_gathers: Generation,
    /// Packages the indexed project defines, to tell repo calls from external ones.
    packages: Arc<ProjectPackages>,
    context: ContextState,
    hover: Hover,
    /// Whether the pointer is over the source view and over the context pane. A hover is
    /// kept while the pointer is over either.
    pointer_in_source: bool,
    pointer_in_pane: bool,
    /// What each pinned context pane shows (`pins.rs`), by pane.
    pins: std::collections::BTreeMap<PaneId, pins::PinView>,
    /// Which list or field has the keyboard (below overlays, forms, pickers and chords).
    key_focus: KeyFocus,
    /// Scroll the selected reference into view on the next render.
    refs_scroll: std::cell::Cell<bool>,
    /// The context pane's rows, built only while on screen: row 0 is the head (declaration,
    /// symbol, definition), then each reference file and reference line.
    context_list: gpui::ListState,
    context_over: crate::overscroll::Overscroll,
    /// What `context_list` was last reset for: the lookup generation and the row count.
    context_list_key: std::cell::Cell<Option<(Generation, usize)>>,
    /// The reference rows behind `context_list` (rows 1 and on).
    context_refs: std::cell::RefCell<std::rc::Rc<Vec<context_pane::RefRow>>>,
    /// Syntax spans of reference previews, highlighted off the UI thread once on screen.
    previews: std::cell::RefCell<context_pane::PreviewSpans>,
    /// The Cmd-K chord in progress, or one held open after GPUI timed it out.
    chord: Chord,
    /// The search panel, which replaces the file list in the sidebar while open.
    search: Option<SearchState>,
    picker: Option<PickerState>,
    /// The Compare picker's background git work (refs, typed commits): a stale answer drops.
    compare_jobs: compare::PickerJobs,
    ranker: Ranker,
    focus: FocusHandle,
    outline_scroll: UniformListScrollHandle,
    outline_over: crate::overscroll::Overscroll,
    /// The Changes list: its rows, selection and keyboard focus.
    changes: changes::ChangesState,
    /// The open investigation (tour or call stack), and the live-reload poll while one is open.
    inv: Option<investigation_view::InvState>,
    inv_watch: Option<gpui::Task<()>>,
    /// The next repaint redraws nothing more (the handler already redrew the one list).
    skip_refresh: bool,
    picker_scroll: UniformListScrollHandle,
    /// Go to file…'s list stretch at its ends.
    picker_over: crate::overscroll::Overscroll,
    /// The plain scrolling areas drawn as they go (dialogs, cards, columns), by name.
    scroll_areas: crate::scroll::ScrollAreas,
    search_scroll: UniformListScrollHandle,
    search_over: crate::overscroll::Overscroll,
    /// Group colours for highlight markers, in the order groups take them.
    marker_colours: Vec<Rgba>,
    /// Which optional features are on (settings, Lightweight mode) and what they cost.
    features: Features,
    /// The layout shell: docks, pane headers and the editor area around the panes.
    shell: Entity<Shell>,
    /// The status bar's cached view.
    status_bar: Entity<index_status::StatusBar>,
    overlay: Option<Overlay>,
    trust: TrustStore,
    /// Build requests checked off the UI thread (projects, trust, the plan): only the newest
    /// one acts when it lands.
    build_checks: Generation,
    job: Option<Job>,
    /// The last build was cancelled; the previous index was kept.
    index_cancelled: bool,
    /// The stored stderr tail of the last failed build, if the cache records one.
    index_failure: Option<String>,
    /// The index banner and "Keep fresh" (manual start: nothing builds until the reader clicks).
    start: index_status::StartState,
    notice: Option<Notice>,
    /// The Copy button of an action step's command that last copied (reads "Copied").
    copied: Option<action_card::Copied>,
    /// The Open Remote form, or an open it started that is still running.
    remote: Option<remote::RemoteState>,
    /// The band above the shell when this window shows one of Chorong's cache worktrees.
    remote_header: Option<remote::HeaderState>,
    /// Prepare and index in a window Open Remote made.
    prepare: Option<prepare::PrepareState>,
    /// The window was opened with "Prepare and index" checked (Update asks again).
    prepare_wanted: bool,
    /// A failed prepare the next index build records: the index then reads `partial`.
    prepare_note: Option<cho_semantic::PrepareNote>,
    /// The prepare path wants one more build once the running one ends.
    rebuild_queued: bool,
    notices: Generation,
    last_esc: Option<Instant>,
    /// Every palette command with its built action, made once.
    command_actions: Vec<(&'static str, &'static str, Box<dyn Action>)>,
    /// The current binding of each command (as in the code view), and the keymap version
    /// it was read from: rebuilt only when the keymap changes (a lookup scans the keymap).
    key_labels: HashMap<&'static str, String>,
    key_labels_for: Option<gpui::KeymapVersion>,
    _subscriptions: Vec<Subscription>,
}

impl Chorong {
    pub fn new(
        theme: Theme,
        editor: Editor,
        font: String,
        fold_by_default: bool,
        mut startup: Startup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let marker_colours = markers::palette(&theme);
        let files = startup.root.as_deref().map(list_files).unwrap_or_default();
        let file_names = files.iter().map(|f| f.display().to_string()).collect();
        let focus = cx.focus_handle();
        let mut features = Features::open_in(cx);
        features.set_repo(
            startup
                .root
                .as_deref()
                .and_then(|r| r.canonicalize().ok())
                .as_deref(),
        );
        // Fonts from settings, before the shell reads them: the bundled IBM Plex unless another
        // installed family is chosen. `--font` on the command line still wins for code.
        let fonts = Self::apply_fonts(&features, cx);
        let cli_font = (!font.is_empty()).then(|| SharedString::from(font));
        let shell = Self::new_shell(&theme, startup.root.as_deref(), &focus, &features, cx);
        let git_folder = startup
            .root
            .as_deref()
            .is_some_and(|r| compare_git::repo_top(r).is_ok());
        let mut this = Self {
            git_folder,
            highlight_uncommitted: false,
            theme,
            editor,
            font: cli_font.clone().unwrap_or(fonts.mono),
            cli_font,
            root: startup.root.take(),
            graph_opts: crate::graph::GraphOptions::load(),
            ref_graph: None,
            graph_building: false,
            graph_panes: Default::default(),
            overlay_keys: HashMap::new(),
            find: Default::default(),
            shift_taps: Default::default(),
            files,
            file_names,
            peek: Default::default(),
            search_full: false,
            note_height: None,
            fold_by_default,
            fold_memory: HashMap::new(),
            lens: false,
            blame_cache: Arc::default(),
            compare: None,
            pending_link: None,
            uncommitted: None,
            uncommitted_gen: Generation::default(),
            links: Vec::new(),
            following: false,
            window_side: Side::After,
            editors: std::collections::BTreeMap::new(),
            active_editor: PaneId(0),
            last_focused: None,
            fallback_editor: EditorState::default(),
            hover_pane: PaneId(0),
            pane_override: false,
            dirty_pane: None,
            dirty_all: false,
            caret_save: None,
            index: IndexState::Absent,
            index_loads: Generation::default(),
            index_id: Generation::default(),
            pin_gathers: Generation::default(),
            packages: Arc::new(ProjectPackages::default()),
            context: ContextState::default(),
            hover: Hover::default(),
            pointer_in_source: false,
            pointer_in_pane: false,
            pins: std::collections::BTreeMap::new(),
            key_focus: KeyFocus::Code,
            refs_scroll: std::cell::Cell::new(false),
            context_list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(200.)),
            context_over: Default::default(),
            context_list_key: std::cell::Cell::new(None),
            context_refs: std::cell::RefCell::default(),
            previews: std::cell::RefCell::default(),
            chord: Chord::Idle,
            search: None,
            picker: None,
            compare_jobs: compare::PickerJobs::default(),
            ranker: Ranker::paths(),
            focus,
            outline_scroll: UniformListScrollHandle::new(),
            outline_over: Default::default(),
            changes: changes::ChangesState::default(),
            inv: None,
            inv_watch: None,
            skip_refresh: false,
            picker_scroll: UniformListScrollHandle::new(),
            picker_over: Default::default(),
            scroll_areas: Default::default(),
            search_scroll: UniformListScrollHandle::new(),
            search_over: Default::default(),
            marker_colours,
            features,
            shell,
            status_bar: {
                let cho = cx.weak_entity();
                cx.new(|_| index_status::StatusBar { cho })
            },
            overlay: None,
            trust: TrustStore::default_location(),
            build_checks: Generation::default(),
            job: None,
            index_cancelled: false,
            index_failure: None,
            start: Default::default(),
            notice: None,
            copied: None,
            remote: None,
            remote_header: None,
            prepare: None,
            prepare_wanted: false,
            prepare_note: None,
            rebuild_queued: false,
            notices: Generation::default(),
            last_esc: None,
            command_actions: all_commands()
                .iter()
                .filter_map(|c| Some((c.action, c.title, cx.build_action(c.action, None).ok()?)))
                .collect(),
            key_labels: HashMap::new(),
            key_labels_for: None,
            _subscriptions: Vec::new(),
        };
        // The Cmd-K popup follows the chord in progress, and holds it open when GPUI's
        // one-second chord timeout drops it before a key was pressed.
        // Pane bodies are cached views. A change that touched only one editor pane says so
        // (`dirty_pane`) and redraws that pane (and the Context and Outline when it is the
        // active one); any other change redraws every pane.
        // The status bar is a cached view too and follows every change. Changes that touch
        // only the chrome (a chord in progress, a notice running out) redraw it alone
        // (`redraw_chrome`), never the panes.
        this._subscriptions.push(cx.observe_self(|this, cx| {
            let dirty = this.dirty_pane.is_some() || this.dirty_all;
            let one = take_refresh_scope(&mut this.dirty_pane, &mut this.dirty_all);
            // Only a list's own view changed (e.g. moving in the Changes list): no pane
            // redraws, unless a change in the same cycle declared one dirty.
            // A pane that shows a new document starts its blame (when the column is on).
            this.ensure_blame(cx);
            if skips_redraw(std::mem::take(&mut this.skip_refresh), dirty) {
                return;
            }
            this.redraw(one.map(shell_host::Redraw::Editor), cx);
            this.redraw_chrome(cx);
            this.schedule_caret_save(cx);
        }));
        // The layout changed: reconcile the editor states and follow focus.
        let shell = this.shell.clone();
        this._subscriptions.push(cx.observe(&shell, |this, _, cx| {
            this.sync_panes(cx);
            this.redraw_chrome(cx);
        }));
        this.sync_panes(cx);
        // The window is back in front: the working folder may have changed meanwhile.
        this._subscriptions
            .push(cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.refresh_uncommitted(cx);
                    this.changes_activated(cx);
                    this.blame_activated(cx);
                    this.remote_header_activated(cx);
                    this.watch_tick(cx);
                }
            }));
        this._subscriptions
            .push(cx.observe_pending_input(window, |this, window, cx| {
                this.pending_input_changed(window);
                this.redraw_chrome(cx);
            }));
        this._subscriptions
            .push(cx.observe_keystrokes(|this, event, _, cx| {
                if this.is_chord_replay(&event.keystroke) {
                    return;
                }
                // A key answered (or abandoned) the chord: nothing to hold.
                if matches!(std::mem::take(&mut this.chord), Chord::Held(_)) {
                    this.redraw_chrome(cx);
                }
            }));
        this.watch_settings(cx);
        this.load_index_in_background(cx);
        if let Some(root) = this.root.as_ref().and_then(|r| r.canonicalize().ok()) {
            // Remembered so links naming this repository by its remote can find it.
            cho_sources::recent::remember(&root).ok();
        }
        let from_remote = matches!(startup.origin, crate::Origin::Remote(_));
        let inv_link = matches!(startup.origin, crate::Origin::Investigation(_));
        this.show_startup(startup, cx);
        // A saved Tour or Call stack pane reopens its investigation, unless a link opens one.
        if !inv_link {
            this.restore_investigation(cx);
        }
        if !from_remote {
            // A window opened later (recents, a link): what the cache recorded.
            this.load_remote_header(cx);
        }
        this
    }

    /// The active editor's state.
    pub(super) fn ed(&self) -> &EditorState {
        self.editors
            .get(&self.active_editor)
            .unwrap_or(&self.fallback_editor)
    }

    pub(super) fn ed_mut(&mut self) -> &mut EditorState {
        self.editors
            .get_mut(&self.active_editor)
            .unwrap_or(&mut self.fallback_editor)
    }

    /// The folder this window shows.
    pub fn root(&self) -> Option<&std::path::Path> {
        self.root.as_deref()
    }

    pub fn show_notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.notify_text(text, cx);
    }

    /// A link from outside Chorong names `folder`, which Chorong has never opened: ask first.
    pub fn ask_open_link(&mut self, url: String, folder: PathBuf, cx: &mut Context<Self>) {
        self.picker = None;
        self.overlay = Some(Overlay::OpenLink { url, folder });
        cx.notify();
    }

    /// A link that arrived while running (from the OS, or a later launch), for the folder
    /// this window shows: jump there, recorded in back/forward history like any other jump.
    pub fn follow(&mut self, startup: Startup, cx: &mut Context<Self>) {
        self.picker = None;
        self.overlay = None;
        self.ed_mut().banner = None;
        self.show_startup(startup, cx);
        cx.notify();
    }

    /// What a window was asked to show (`new`) or a link that arrived later asks for
    /// (`follow`): the origin's banner, question or comparison, and the file at its place.
    fn show_startup(&mut self, startup: Startup, cx: &mut Context<Self>) {
        use crate::Origin;
        let Startup {
            file,
            extra_files,
            symbol,
            line,
            origin,
            compare,
            ..
        } = startup;
        let (link, remote) = match origin {
            Origin::Plain => (None, None),
            Origin::Link(info) => (Some(info), None),
            Origin::Elsewhere { info, web } => {
                self.ed_mut().banner = Some(elsewhere_banner(&info, web));
                (None, None)
            }
            Origin::AskPr(ask) => {
                self.overlay = Some(Overlay::OpenPullRequest(ask));
                (None, None)
            }
            Origin::Remote(mut r) => (r.link.take(), Some(r)),
            Origin::Investigation(source) => {
                self.open_investigation(source, cx);
                (None, None)
            }
        };
        let from_link = link.is_some();
        let view_link = link.clone().filter(LinkInfo::wants_comparison);
        if let Some(info) = link {
            self.link_opened(info, cx);
        }
        if let Some(file) = file {
            let target = Target {
                from_link,
                ..Target::at(symbol, line)
            };
            // A split already showing this pull request: the link's side takes focus (never
            // in a new window, which has no split yet).
            let side_focused = view_link
                .as_ref()
                .is_some_and(|info| self.focus_split_side(info, cx));
            if !side_focused {
                self.open(file.clone(), target, true, cx);
                if let Some(info) = &view_link {
                    self.expect_link_side(info, file);
                }
            }
        }
        // Files given together with it go to the focus layout's tray.
        self.open_files_in_tray(extra_files, cx);
        if let Some(r) = remote {
            self.begin_prepare(r.prepare, cx);
            // Straight from the open that made this window: no cache read.
            self.remote_header = Some(remote::HeaderState::new(r.header));
            self.check_pr_head(cx);
            let notes = r.notes.join(" ");
            self.start_comparison(
                r.before,
                r.against,
                r.merge_bases,
                Some(notes).filter(|n| !n.is_empty()),
                cx,
            );
        } else if let Some(info) = &view_link {
            self.start_link_comparison(info, cx);
        } else if let Some(rev) = compare {
            // `chorong-app PATH --compare REF`: as Compare Against… with REF chosen.
            self.choose_comparison(rev.clone(), rev, cx);
        }
    }

    /// Gives the keyboard to `focus`, taking it from whichever list or field had it.
    fn focus_to(&mut self, focus: KeyFocus) {
        self.key_focus = focus;
    }

    /// Keystrokes go to the search query.
    fn search_focused(&self) -> bool {
        self.key_focus == KeyFocus::Search && self.search.is_some()
    }

    /// `title (key)` for a tooltip, from the live keymap.
    fn tip_text(&self, title: &str, action: &str) -> String {
        match self.key_labels.get(action) {
            Some(k) => format!("{title} ({k})"),
            None => title.to_string(),
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_chord_replay(&event.keystroke) {
            return;
        }
        if let Chord::Held(mut keys) = std::mem::take(&mut self.chord) {
            // The next key finishes the held chord, looked up in the live keymap. A key that
            // follows nothing in the popup just closes it.
            keys.push(event.keystroke.clone());
            if let Some(action) = Self::chord_action(&keys, cx) {
                window.dispatch_action(action, cx);
            }
            cx.stop_propagation();
            self.redraw_chrome(cx);
            return;
        }
        if self.remote_visible() {
            self.remote_key(event, cx);
        } else if self.picker.as_ref().is_some_and(|p| {
            let k = &event.keystroke;
            p.add_to_page && p.query.is_empty() && k.key == "backspace" && !k.modifiers.modified()
        }) && self.tray_to_return_to(cx)
        {
            // Backspace on an empty add-to-page query removes the Files chip: back to Bring
            // from tray… (when the tray has anything to bring).
            self.back_to_tray(window, cx);
            cx.stop_propagation();
        } else if self.picker.as_ref().is_some_and(|p| {
            let k = &event.keystroke;
            p.tour_scope && p.query.is_empty() && k.key == "backspace" && !k.modifiers.modified()
        }) {
            // Backspace on an empty query takes the Tour chip off: every file.
            self.toggle_picker_tour_scope();
            cx.stop_propagation();
            cx.notify();
        } else if self.picker.is_none() && self.find_focused() {
            let k = &event.keystroke;
            let paste = (k.key == "v" && k.modifiers.secondary())
                .then(|| cx.read_from_clipboard().and_then(|c| c.text()))
                .flatten();
            if !self.find_key(k, paste.as_deref(), cx) {
                return;
            }
            cx.stop_propagation();
            cx.notify();
        } else if self.picker.is_some() || self.search_focused() {
            let k = &event.keystroke;
            let paste = (k.key == "v" && k.modifiers.secondary())
                .then(|| cx.read_from_clipboard().and_then(|c| c.text()))
                .flatten();
            let in_picker = self.picker.is_some();
            let edit = match (&mut self.picker, &mut self.search) {
                (Some(p), _) => edit_field(k, &mut p.query, &mut p.query_all, paste.as_deref()),
                (None, Some(s)) => edit_field(k, &mut s.query, &mut s.query_all, paste.as_deref()),
                (None, None) => FieldEdit::None,
            };
            match edit {
                FieldEdit::None => return,
                FieldEdit::Text if in_picker => self.requery(cx),
                FieldEdit::Text => self.schedule_search(cx),
                FieldEdit::Selection => {}
                FieldEdit::Copy(text) => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text))
                }
            }
            cx.stop_propagation();
            cx.notify();
        } else if self.key_focus == KeyFocus::Changes {
            // Typing in the Changes list filters it.
            let k = &event.keystroke;
            let paste = (k.key == "v" && k.modifiers.secondary())
                .then(|| cx.read_from_clipboard().and_then(|c| c.text()))
                .flatten();
            match self.changes_filter_key(k, paste.as_deref()) {
                FieldEdit::None => return,
                FieldEdit::Copy(text) => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text))
                }
                FieldEdit::Text | FieldEdit::Selection => {}
            }
            cx.stop_propagation();
            self.refresh_changes_panes(cx);
            self.skip_refresh = true;
        }
    }

    fn open_in_editor(&mut self, cx: &mut Context<Self>) {
        // Chorong never sends an editor into its own cache: those views offer the host's page.
        if self.remote_header.is_some() {
            self.open_on_host(cx);
            return;
        }
        let Some(doc) = &self.ed().doc else { return };
        let mut loc = EditorLocation::new(doc.path.clone());
        if let Some(line) = self.selected_line() {
            // On Before the working copy opens at the matching line, mapped through the diff.
            let line = self.working_line(line);
            loc = loc.at(line + 1, 1);
        }
        let launch = match &self.root {
            Some(root) => self.editor.open_project(root, Some(&loc)),
            None => self.editor.open_file(&loc),
        };
        let Launch::Command { program, args } = &launch;
        let result = std::process::Command::new(program)
            .args(args)
            .spawn()
            .map(drop);
        if let Err(e) = result {
            self.ed_mut().error = Some(format!("could not run `{launch}`: {e}"));
        }
    }
}

/// Which pane bodies a change redraws: the one editor pane it declared (`dirty_pane`, with
/// Context and Outline when that is the active editor), or every pane when it declared none
/// or the active editor changed (`dirty_all`). Every refresh consumes both flags, so an
/// "everything" left over from one change never widens the next.
fn take_refresh_scope(dirty_pane: &mut Option<PaneId>, dirty_all: &mut bool) -> Option<PaneId> {
    let all = std::mem::take(dirty_all);
    dirty_pane.take().filter(|_| !all)
}

/// Whether a repaint leaves the panes alone: a list's own navigation asked for it
/// (`skip_refresh`), and no change in the same cycle declared a pane dirty. A dirty flag always
/// wins, so a swallowed redraw cannot leave a pane stale.
fn skips_redraw(skip_refresh: bool, dirty: bool) -> bool {
    skip_refresh && !dirty
}

/// The root key context while a find bar's query has the keyboard (for the keymap's tests).
#[cfg(test)]
pub(crate) const FIND_CONTEXT: &str = find_bar::CONTEXT;

/// Every palette command, from each module's `COMMANDS`, in `rank` order. This one list feeds
/// the palette, the cheat sheet and the user guide's key table.
pub(crate) fn all_commands() -> &'static [Command] {
    static ALL: std::sync::OnceLock<Vec<Command>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut all: Vec<Command> = [
            blame::COMMANDS,
            changes::COMMANDS,
            commands::COMMANDS,
            compare::COMMANDS,
            comparison_pane::COMMANDS,
            context_pane::COMMANDS,
            editor::COMMANDS,
            graph_ui::COMMANDS,
            highlights::COMMANDS,
            index_status::COMMANDS,
            investigation_view::COMMANDS,
            tour_pages_view::COMMANDS,
            navigation::COMMANDS,
            picker_ui::COMMANDS,
            pins::COMMANDS,
            remote::COMMANDS,
            search_pane::COMMANDS,
            settings_cmds::COMMANDS,
            shell_host::COMMANDS,
            overlays::COMMANDS,
        ]
        .into_iter()
        .flat_map(|s| s.iter().copied())
        .collect();
        all.sort_by_key(|c| c.rank);
        all
    })
}

impl Render for Chorong {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Here, before the shell draws (its pane bodies must not read it).
        self.settle_search_full(cx);
        // The preview follows the selection (it reads only once the selection rests).
        self.sync_peek(cx);
        // Keys shown anywhere (palette, tooltips, cheat sheet) are read from the live keymap,
        // again whenever it changes.
        let keymap = cx.key_bindings().borrow().version();
        if self.key_labels_for != Some(keymap) {
            self.key_labels_for = Some(keymap);
            self.key_labels = self
                .command_actions
                .iter()
                .filter_map(|(name, _, action)| {
                    Some((*name, Self::binding_label(window, action.as_ref())?))
                })
                .collect();
        }
        self.prepare_find(window, cx);
        self.start_late_folds(cx);
        // The keys an overlay shows in its buttons and headings, as bound in its own context.
        let overlay = self.overlay.as_ref().map(Overlay::kind);
        if let Some(kind) = overlay.filter(|k| !k.shown_actions().is_empty()) {
            let names = kind.shown_actions();
            let ctx = KeyContext::parse(kind.key_context()).ok();
            self.overlay_keys = names
                .iter()
                .copied()
                .filter_map(|name| {
                    let action = cx.build_action(name, None).ok()?;
                    let b = window.highest_precedence_binding_for_action_in_context(
                        action.as_ref(),
                        ctx.clone()?,
                    )?;
                    Some((name, keymap::label(&b, Platform::current())))
                })
                .collect();
        }
        let context = if let Some(kind) = overlay {
            kind.key_context().to_string()
        } else if self.remote_visible() {
            "Chorong Remote".to_string()
        } else if let Some(p) = &self.picker {
            match p.kind {
                PickerKind::Palette => "Chorong Picker Palette".to_string(),
                PickerKind::Files if p.add_to_page => "Chorong Picker AddToPage".to_string(),
                _ => "Chorong Picker".to_string(),
            }
        } else if matches!(self.chord, Chord::Held(_)) {
            // No single-key bindings while a held chord waits for its key.
            "Chorong Chord".to_string()
        } else if self.shell.read(cx).overlay_open() {
            // A shell picker (Go to pane…, Add to page from the tray…) types a query: its own
            // context sits deeper, but a `CodeView` here would still match the code view's
            // single-letter keys and take the letters before the query sees them.
            "Chorong PaneOverlay".to_string()
        } else {
            // Single-letter keys exist only in the code view: never in pickers, dialogs or
            // text fields. While the index banner is up, Esc in the code presses its Not now
            // (`IndexBanner` in the keymap); Enter keeps its meaning in the code.
            let mut c = self.key_focus.key_context().to_string();
            if self.key_focus == KeyFocus::Code && self.start_banner().is_some() {
                c.push_str(" IndexBanner");
            }
            c
        };
        let hint_bar = self
            .shell
            .update(cx, |shell, cx| shell.hint_bar(window, cx));
        let pending: Vec<Keystroke> = window
            .pending_input_keystrokes()
            .filter(|p| !p.is_empty())
            .map(|p| p.to_vec())
            .or_else(|| match &self.chord {
                Chord::Held(keys) => Some(keys.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let chord = Self::chord_followers(&pending, cx);
        let chord_prefix = pending
            .iter()
            .map(|k| keymap::stroke_label(k, Platform::current()))
            .collect::<Vec<_>>()
            .join(" ");
        let d = div().key_context(KeyContext::parse(&context).unwrap_or_default());
        let d = d.track_focus(&self.focus);
        let d = blame::register(d, cx);
        let d = changes::register(d, cx);
        let d = commands::register(d, cx);
        let d = compare::register(d, cx);
        let d = comparison_pane::register(d, cx);
        let d = context_pane::register(d, cx);
        let d = editor::register(d, cx);
        let d = find_bar::register(d, cx);
        let d = graph_ui::register(d, cx);
        let d = highlights::register(d, cx);
        let d = index_status::register(d, cx);
        let d = investigation_view::register(d, cx);
        let d = tour_pages_view::register(d, cx);
        let d = navigation::register(d, cx);
        let d = overlays::register(d, cx);
        let d = picker_ui::register(d, cx);
        let d = pins::register(d, cx);
        let d = remote::register(d, cx);
        let d = search_pane::register(d, cx);
        let d = settings_cmds::register(d, cx);
        let d = shell_host::register(d, cx);
        // Text entry is the only thing handled on raw key presses.
        let d = d.on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            this.on_key_down(event, window, cx);
        }));
        // Shift twice quickly opens the command palette; any key in between cancels it.
        let d = d
            .capture_key_down(cx.listener(|this, _: &KeyDownEvent, _, _| this.shift_taps.key()))
            .on_modifiers_changed(cx.listener(|this, e: &gpui::ModifiersChangedEvent, _, cx| {
                let now = std::time::Instant::now();
                if this.shift_taps.modifiers(&e.modifiers, now)
                    && this.picker.is_none()
                    && this.overlay.is_none()
                {
                    this.open_palette();
                    cx.notify();
                }
            }));
        // A mouse's back and forward buttons (and a three-finger swipe, where macOS sends one)
        // go back and forward in the active code pane.
        let d = d
            .on_mouse_down(
                MouseButton::Navigate(gpui::NavigationDirection::Back),
                cx.listener(|this, _, _, cx| {
                    this.go_back(cx);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(gpui::NavigationDirection::Forward),
                cx.listener(|this, _, _, cx| {
                    this.go_forward(cx);
                    cx.notify();
                }),
            );
        // The layout shell's actions (docks, panes, Layout mode) reach it from anywhere under
        // the viewer's root.
        bind_actions(d, &self.shell)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(cx.global::<cho_fonts::Fonts>().ui.clone())
            .bg(hsla(self.theme.background))
            .text_color(hsla(self.theme.text))
            .children(self.render_remote_header(cx))
            .child(self.shell.clone())
            .children(hint_bar.map(|bar| {
                // Layout mode's hint bar, in a strip of its own just above the status bar.
                div()
                    .flex_none()
                    .h(px(24.))
                    .px_3()
                    .flex()
                    .items_center()
                    .bg(hsla(self.theme.status_bar_background))
                    .border_t_1()
                    .border_color(hsla(self.theme.border))
                    .text_size(px(12.))
                    .text_color(hsla(self.theme.text_muted))
                    .child(bar)
            }))
            .child(self.status_bar_view())
            .children(self.render_start_banner(cx))
            .when(!chord.is_empty(), |d| {
                d.child(self.render_chord_popup(&chord, chord_prefix))
            })
            .children(
                self.picker
                    .as_ref()
                    .map(|picker| self.render_picker(picker, window, cx)),
            )
            .children(
                self.remote
                    .as_ref()
                    .filter(|r| r.visible)
                    .map(|r| self.render_remote(r, cx)),
            )
            .children(
                self.prepare
                    .as_ref()
                    .and_then(|p| p.failure.as_ref())
                    .map(|f| self.render_prepare_failure(f, cx)),
            )
            .children(match &self.overlay {
                Some(Overlay::Cheat) => Some(self.render_cheat_sheet(cx).into_any_element()),
                Some(Overlay::Trust { notes }) => {
                    Some(self.render_trust(notes, cx).into_any_element())
                }
                Some(Overlay::OpenLink { url, folder }) => {
                    Some(self.render_open_link(url, folder, cx).into_any_element())
                }
                Some(Overlay::OpenPullRequest(ask)) => {
                    Some(self.render_open_pull_request(ask, cx).into_any_element())
                }
                Some(Overlay::IndexError) => Some(self.render_index_error(cx).into_any_element()),
                Some(Overlay::PrepareApproval(p)) => {
                    Some(self.render_prepare_approval(p, cx).into_any_element())
                }
                Some(Overlay::Graph(p)) => {
                    Some(self.render_graph_popover(p, cx).into_any_element())
                }
                None => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_redraws_its_pane_unless_everything_changed() {
        let (mut one, mut all) = (Some(PaneId(3)), false);
        assert_eq!(take_refresh_scope(&mut one, &mut all), Some(PaneId(3)));
        assert_eq!((one, all), (None, false));
        let (mut one, mut all) = (Some(PaneId(3)), true);
        assert_eq!(take_refresh_scope(&mut one, &mut all), None);
        assert_eq!(take_refresh_scope(&mut None, &mut false), None);
    }

    /// An undeclared change redraws everything and consumes `dirty_all` too: the next
    /// one-pane change redraws only that pane (it used to stay "everything" for good).
    #[test]
    fn everything_is_consumed_by_the_refresh_that_uses_it() {
        let (mut one, mut all) = (None, true);
        assert_eq!(take_refresh_scope(&mut one, &mut all), None);
        assert!(!all);
        one = Some(PaneId(8));
        assert_eq!(take_refresh_scope(&mut one, &mut all), Some(PaneId(8)));
    }

    /// Show Changes while the search query had the keyboard used to leave the keys in Search
    /// (and in Changes once search closed): the last list asked for has the keyboard, and a
    /// list letting go only takes it back from itself.
    #[test]
    fn the_last_list_asked_for_has_the_keyboard() {
        let mut focus = KeyFocus::Search;
        assert_eq!(focus.key_context(), "Chorong Search");
        focus = KeyFocus::Changes; // Show Changes
        assert_eq!(focus.key_context(), "Chorong Changes");
        focus.leave(KeyFocus::Search); // the search panel closes
        assert_eq!(focus, KeyFocus::Changes);
        focus = KeyFocus::References { row: 3 };
        assert_eq!(focus.reference_row(), Some(3));
        focus.leave(KeyFocus::Changes);
        assert_eq!(focus.key_context(), "Chorong References");
        focus.leave(KeyFocus::References { row: 0 }); // any row
        assert_eq!(focus, KeyFocus::Code);
        assert_eq!(focus.key_context(), "Chorong CodeView");
    }
}
