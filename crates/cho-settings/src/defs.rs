//! Every setting: its stable key, default, description, cost hint and cost class.
//!
//! This table is what the settings page renders and what the CLI prints. It mirrors the table
//! in `docs/public/settings.md`; a test keeps the two in step.

use std::borrow::Cow;
use std::fmt;

/// A setting's value. Booleans are toggles; unsigned integers are limits and delays; text is
/// a name, such as a font family.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    UInt(u64),
    Text(Cow<'static, str>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(b) => b.fmt(f),
            Value::UInt(n) => n.fmt(f),
            Value::Text(t) => t.fmt(f),
        }
    }
}

impl Value {
    /// The JSON text of this value.
    pub fn literal(&self) -> String {
        match self {
            Value::Text(t) => crate::jsonc::quote(t),
            other => other.to_string(),
        }
    }

    /// A text value.
    pub fn text(s: impl Into<String>) -> Self {
        Value::Text(Cow::Owned(s.into()))
    }
}

/// Longest text value accepted (a font family name).
pub const MAX_TEXT: usize = 200;

/// What a setting holds, and what values are allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    UInt {
        min: u64,
        max: u64,
    },
    /// A font family name; `mono` when it is for code. The settings page offers the installed
    /// families; any non-empty name is accepted, and one that is not installed falls back.
    Font {
        mono: bool,
    },
    /// An absolute path to a program or folder, or empty for "not set" (Chorong then looks for
    /// it, or uses its own copy).
    Path,
    /// An npm version spec for an indexer: an exact version, `^x.y.z`, `~x.y.z` or `latest`
    /// ([`cho_core::npm_version::Spec`]).
    Version,
    /// One of a few words, in order from least to most; the first is "off" (what Lightweight
    /// mode chooses).
    Choice {
        options: &'static [&'static str],
    },
}

/// Longest path value accepted.
pub const MAX_PATH: usize = 4096;

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Bool => "bool",
            Kind::UInt { .. } => "uint",
            Kind::Font { .. } => "font",
            Kind::Path => "path",
            Kind::Version => "version",
            Kind::Choice { .. } => "choice",
        }
    }

    /// `Ok` if `v` is this kind and in range; otherwise why not.
    pub fn check(&self, v: &Value) -> Result<(), String> {
        match (self, v) {
            (Kind::Bool, Value::Bool(_)) => Ok(()),
            (Kind::UInt { min, max }, Value::UInt(n)) if (min..=max).contains(&n) => Ok(()),
            (Kind::UInt { min, max }, Value::UInt(n)) => {
                Err(format!("{n} is out of range ({min} to {max})"))
            }
            (Kind::Font { .. }, Value::Text(t)) if !t.trim().is_empty() && t.len() <= MAX_TEXT => {
                Ok(())
            }
            (Kind::Font { .. }, Value::Text(_)) => Err(format!(
                "expected a font family name (1 to {MAX_TEXT} characters)"
            )),
            (Kind::Path, Value::Text(t))
                if t.is_empty()
                    || (std::path::Path::new(t.as_ref()).is_absolute()
                        && t.len() <= MAX_PATH
                        && !t.contains('\n')) =>
            {
                Ok(())
            }
            (Kind::Path, Value::Text(_)) => {
                Err("expected an absolute path, or \"\" for not set".into())
            }
            (Kind::Version, Value::Text(t)) => cho_core::npm_version::Spec::parse(t).map(|_| ()),
            (Kind::Choice { options }, Value::Text(t)) if options.contains(&t.as_ref()) => Ok(()),
            (Kind::Choice { options }, _) => Err(format!("expected one of {}", options.join(", "))),
            (Kind::Bool, _) => Err("expected true or false".into()),
            (Kind::UInt { min, max }, _) => {
                Err(format!("expected a whole number from {min} to {max}"))
            }
            (Kind::Font { .. }, _) => Err("expected a font family name, in quotes".into()),
            (Kind::Path, _) => Err("expected an absolute path in quotes, or \"\"".into()),
            (Kind::Version, _) => {
                Err("expected a version in quotes (0.6.0, ^0.6.0, ~0.6.0 or latest)".into())
            }
        }
    }

    /// Parse command-line or file text as a value of this kind (range checked).
    pub fn parse(&self, text: &str) -> Result<Value, String> {
        let v = match self {
            Kind::Bool => match text {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err(format!("`{text}` is not true or false")),
            },
            Kind::UInt { .. } => match text.parse::<u64>() {
                Ok(n) => Value::UInt(n),
                Err(_) => return Err(format!("`{text}` is not a whole number")),
            },
            Kind::Font { .. } | Kind::Path | Kind::Version | Kind::Choice { .. } => {
                Value::text(text.trim())
            }
        };
        self.check(&v).map(|()| v)
    }
}

/// How a feature spends its time, for the cost hint and the page's sorting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostClass {
    /// Runs in the background and can use real CPU, memory or disk (the indexer).
    BackgroundHeavy,
    /// Runs when the pointer or caret moves or rests.
    PerPointer,
    /// Runs every frame, or whenever the view changes.
    PerFrame,
    /// Does nothing until the reader asks.
    OnDemand,
    /// Too small to measure; a preference.
    Negligible,
}

impl CostClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            CostClass::BackgroundHeavy => "background-heavy",
            CostClass::PerPointer => "per-pointer",
            CostClass::PerFrame => "per-frame",
            CostClass::OnDemand => "on-demand",
            CostClass::Negligible => "negligible",
        }
    }
}

/// Everything known about one setting except its current value.
#[derive(Clone, Debug)]
pub struct SettingDef {
    /// Stable dotted key, as written in `settings.json`.
    pub key: &'static str,
    /// One line, shown on the page.
    pub description: &'static str,
    /// What kind of work it does and when.
    pub cost_hint: &'static str,
    pub cost_class: CostClass,
    /// Lightweight mode resolves this setting to off ([`SettingDef::off_value`]). Only toggles
    /// and choices can be.
    pub lightweight_off: bool,
    pub kind: Kind,
    pub default: Value,
}

impl SettingDef {
    /// What Lightweight mode sets: `false`, or a choice's first option.
    pub fn off_value(&self) -> Value {
        match self.kind {
            Kind::Choice { options } => Value::Text(Cow::Borrowed(options[0])),
            _ => Value::Bool(false),
        }
    }

    /// The part of the key before the first dot, for grouping on the page.
    pub fn group(&self) -> &'static str {
        self.key.split_once('.').map_or("general", |(g, _)| g)
    }
}

/// Conversion between a typed field and [`Value`].
pub trait SettingType: Sized {
    fn from_value(v: &Value) -> Option<Self>;
    fn to_value(&self) -> Value;
}

impl SettingType for bool {
    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn to_value(&self) -> Value {
        Value::Bool(*self)
    }
}

impl SettingType for u32 {
    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::UInt(n) => u32::try_from(*n).ok(),
            _ => None,
        }
    }
    fn to_value(&self) -> Value {
        Value::UInt(u64::from(*self))
    }
}

impl SettingType for String {
    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Text(t) => Some(t.to_string()),
            _ => None,
        }
    }
    fn to_value(&self) -> Value {
        Value::text(self.clone())
    }
}

macro_rules! define_settings {
    ($(
        $field:ident : $ty:ty = $default:expr;
        key $key:literal;
        kind $kind:expr;
        class $class:ident;
        lightweight $lw:literal;
        desc $desc:literal;
        cost $cost:literal;
    )*) => {
        /// All settings as typed fields. Field names are the dotted keys with `_` for `.`.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct Settings {
            $( pub $field: $ty, )*
        }

        /// The table of settings, in display order.
        pub static DEFS: &[SettingDef] = &[
            $( SettingDef {
                key: $key,
                description: $desc,
                cost_hint: $cost,
                cost_class: CostClass::$class,
                lightweight_off: $lw,
                kind: $kind,
                default: $default,
            }, )*
        ];

        impl Settings {
            /// The value stored under `key`, or `None` if there is no such setting.
            pub fn get(&self, key: &str) -> Option<Value> {
                match key {
                    $( $key => Some(SettingType::to_value(&self.$field)), )*
                    _ => None,
                }
            }

            /// Store `value` under `key`. Does not range-check; use [`Kind::check`] first.
            pub fn set(&mut self, key: &str, value: Value) -> bool {
                match key {
                    $( $key => match <$ty as SettingType>::from_value(&value) {
                        Some(v) => { self.$field = v; true }
                        None => false,
                    }, )*
                    _ => false,
                }
            }
        }
    };
}

define_settings! {
    lightweight: bool = Value::Bool(false);
    key "lightweight";
    kind Kind::Bool;
    class Negligible;
    lightweight false;
    desc "Lightweight mode: turn every optional feature off at once";
    cost "None. Restores each feature's own setting when turned off.";

    changes_auto_refresh: bool = Value::Bool(true);
    key "changes.auto_refresh";
    kind Kind::Bool;
    class BackgroundHeavy;
    lightweight true;
    desc "Refresh the Changes list when the window is activated";
    cost "Re-diffs the whole comparison in the background on each window activation: about 15 ms on a small repository, about 2 s on a large one (measured in a debug build). Off: the list refreshes only on Compare Against, Show Changes and its Refresh button.";

    compare_line_highlight: String = Value::Text(Cow::Borrowed("subtle"));
    key "compare.line_highlight";
    kind Kind::Choice { options: &["off", "subtle", "stronger"] };
    class PerFrame;
    lightweight true;
    desc "While comparing: tint changed lines and draw a rule where lines were removed or added (off, subtle or stronger)";
    cost "Draws a background and at most one rule per visible line, from marks worked out once per file and comparison: under 1 us a line. Scrolling a 1,300-line file with a large diff gave the same frame times off, subtle and stronger (p50 about 5.4 ms, p95 about 6.5 ms, debug build).";

    compare_changed_words: bool = Value::Bool(true);
    key "compare.changed_words";
    kind Kind::Bool;
    class PerFrame;
    lightweight true;
    desc "While comparing: a darker block on the words that differ within a changed line";
    cost "Finds changed words in the background with the line diff when a file is prepared (about 40 ms in a debug build for a 1,300-line file with a large rewrite), then draws one box per changed word on visible lines: under 1 us a line.";

    compare_follow_highlight: String = Value::Text(Cow::Borrowed("fade"));
    key "compare.follow_highlight";
    kind Kind::Choice { options: &["off", "outline", "fade"] };
    class PerFrame;
    lightweight true;
    desc "In a linked split, briefly highlight the declaration the other column follows to: off, a still outline, or a fade";
    cost "Redraws the followed column for 0.9 s after each follow: about 27 frames while it fades, one after the outline. A frame costs what a normal redraw costs.";

    context_follow_hover: bool = Value::Bool(true);
    key "context.follow_hover";
    kind Kind::Bool;
    class PerPointer;
    lightweight true;
    desc "Context pane follows the word under the pointer";
    cost "A lookup each time the pointer rests on a word (after context.hover_delay_ms). Small per lookup; adds up when moving the pointer a lot.";

    context_hover_delay_ms: u32 = Value::UInt(300);
    key "context.hover_delay_ms";
    kind Kind::UInt { min: 0, max: 5000 };
    class Negligible;
    lightweight false;
    desc "How long the pointer rests on a word before the context pane follows it (ms)";
    cost "A longer delay means fewer lookups while moving the pointer.";

    context_follow_caret: bool = Value::Bool(true);
    key "context.follow_caret";
    kind Kind::Bool;
    class PerPointer;
    lightweight true;
    desc "Context pane follows the caret";
    cost "A lookup per caret move. Small.";

    context_references: bool = Value::Bool(true);
    key "context.references";
    kind Kind::Bool;
    class PerPointer;
    lightweight true;
    desc "Context pane lists references";
    cost "Reads a preview line from each referencing file, in the background. Grows with the number of references.";

    hover_card: bool = Value::Bool(true);
    key "hover.card";
    kind Kind::Bool;
    class PerPointer;
    lightweight true;
    desc "Hover card with type, signature and docs";
    cost "One index lookup per hover. Small.";

    hover_peek: bool = Value::Bool(true);
    key "hover.peek";
    kind Kind::Bool;
    class PerPointer;
    lightweight true;
    desc "Definition peek in the hover card";
    cost "Parses the defining file's outline once (cached). The first peek in a large file costs the most.";

    highlight_exact: bool = Value::Bool(true);
    key "highlight.exact";
    kind Kind::Bool;
    class OnDemand;
    lightweight true;
    desc "Exact (index) mode for double-click highlights";
    cost "One index lookup per highlight. Small; off falls back to name matching.";

    highlight_gutter_marks: bool = Value::Bool(true);
    key "highlight.gutter_marks";
    kind Kind::Bool;
    class Negligible;
    lightweight false;
    desc "Mark highlighted lines in the gutter";
    cost "Negligible: drawn from the same list as the highlight boxes.";

    lens_call_kinds: bool = Value::Bool(true);
    key "lens.call_kinds";
    kind Kind::Bool;
    class PerFrame;
    lightweight true;
    desc "Critical Lens colours calls by kind";
    cost "One index lookup per call on screen when the lens is on. Grows with calls per screen.";

    // Manual start (design/manual-start.md): no build starts until the reader clicks.
    // `index.auto_build` and `prepare.auto` are retired (see `RETIRED`).
    index_auto_refresh: bool = Value::Bool(false);
    key "index.auto_refresh";
    kind Kind::Bool;
    class BackgroundHeavy;
    lightweight true;
    desc "Keep the index fresh in every session: refresh a stale index without asking (the banner's \"Keep fresh this session\" for one window)";
    cost "Heavy, in the background, each time the code changes enough to make the index stale: runs the indexer, which runs the project's own code (Rust build scripts and macros).";

    index_low_priority: bool = Value::Bool(true);
    key "index.low_priority";
    kind Kind::Bool;
    class OnDemand;
    lightweight false;
    desc "Run indexers below the viewer's priority (nice 10)";
    cost "None while reading. On, a build yields the CPU to the viewer; off, it finishes sooner but can make the viewer stutter.";

    index_banner: bool = Value::Bool(true);
    key "index.banner";
    kind Kind::Bool;
    class Negligible;
    lightweight false;
    desc "Show the index banner when there is no index or it is out of date";
    cost "None: a card drawn while the index is missing or stale. Off: build or refresh from the status bar's Index item, Cmd-K I or the palette.";
    // End of the manual-start block.

    index_parallel_projects: u32 = Value::UInt(1);
    key "index.parallel_projects";
    kind Kind::UInt { min: 1, max: 32 };
    class BackgroundHeavy;
    lightweight false;
    desc "How many project indexes build at once";
    cost "More is faster to finish but competes with reading for CPU.";

    index_node_memory_mb: u32 = Value::UInt(8192);
    key "index.node_memory_mb";
    kind Kind::UInt { min: 512, max: 65536 };
    class BackgroundHeavy;
    lightweight false;
    desc "Memory limit for the TypeScript indexer's Node process (MB)";
    cost "Raises the ceiling only; a large pnpm or Turborepo monorepo can use most of it while indexing (several GB). A NODE_OPTIONS you set yourself with --max-old-space-size wins.";

    index_typescript_version: String = Value::Text(Cow::Borrowed("0.6.0"));
    key "index.typescript_version";
    kind Kind::Version;
    class OnDemand;
    lightweight false;
    desc "Version of scip-typescript Chorong installs and runs: exact (0.6.0), a range (^0.6.0, ~0.6.0) or latest";
    cost "Nothing until a build. A version Chorong has not installed yet is downloaded once at the start of the next build (36 MB on disk with its TypeScript; seconds); installed versions are kept in Chorong's cache.";

    index_python_version: String = Value::Text(Cow::Borrowed("0.6.8"));
    key "index.python_version";
    kind Kind::Version;
    class OnDemand;
    lightweight false;
    desc "Version of scip-python Chorong installs and runs: exact (0.6.8), a range (^0.6.8, ~0.6.8) or latest";
    cost "Nothing until a build. A version Chorong has not installed yet is downloaded once at the start of the next build (73 MB on disk; seconds); installed versions are kept in Chorong's cache.";

    index_version_check: bool = Value::Bool(true);
    key "index.version_check";
    kind Kind::Bool;
    class BackgroundHeavy;
    lightweight true;
    desc "Check the registry for a newer match of a range or latest indexer version, every index.version_check_days";
    cost "One `npm view` (a registry request, under 1 s) when a build starts and the last check is older than index.version_check_days; nothing for an exact version. Off: the installed match is used until the setting changes.";

    index_version_check_days: u32 = Value::UInt(7);
    key "index.version_check_days";
    kind Kind::UInt { min: 1, max: 365 };
    class OnDemand;
    lightweight false;
    desc "Days between registry checks for a range or latest indexer version";
    cost "Fewer days means more registry requests, each under 1 s at the start of a build.";

    index_typescript_install: String = Value::Text(Cow::Borrowed(""));
    key "index.typescript_install";
    kind Kind::Path;
    class Negligible;
    lightweight false;
    desc "Your own scip-typescript (its package folder or its main script) instead of Chorong's copy; Chorong then downloads nothing";
    cost "None: replaces the download.";

    index_python_install: String = Value::Text(Cow::Borrowed(""));
    key "index.python_install";
    kind Kind::Path;
    class Negligible;
    lightweight false;
    desc "Your own scip-python (its package folder or its index.js) instead of Chorong's copy; Chorong then downloads nothing";
    cost "None: replaces the download.";

    investigations_watch: bool = Value::Bool(true);
    key "investigations.watch";
    kind Kind::Bool;
    class Negligible;
    lightweight true;
    desc "Watch the open tour or call stack file and offer Updated \u{b7} Reload when it changes";
    cost "While an investigation is open: one file stat every 2 s and on window activation (about 0.5 µs each, measured in a debug build on Linux), and one read and parse of that file when it changed. Off: edits to the file are not noticed until you open it again.";

    tools_node: String = Value::Text(Cow::Borrowed(""));
    key "tools.node";
    kind Kind::Path;
    class OnDemand;
    lightweight false;
    desc "Path to node, which runs the TypeScript and Python indexers. Empty: found on first use and saved here";
    cost "Empty: one login-shell start (under 1 s, 5 s at most) and a look in known places, the first time an indexer needs it; then nothing.";

    tools_npm: String = Value::Text(Cow::Borrowed(""));
    key "tools.npm";
    kind Kind::Path;
    class OnDemand;
    lightweight false;
    desc "Path to npm, which installs Chorong's copies of the indexers. Empty: found on first use and saved here";
    cost "Empty: found with node (the same login-shell start), the first time an indexer is installed; then nothing.";

    tools_uv: String = Value::Text(Cow::Borrowed(""));
    key "tools.uv";
    kind Kind::Path;
    class OnDemand;
    lightweight false;
    desc "Path to uv, which a Python project's prepare recipe runs. Empty: found on first use and saved here";
    cost "Empty: one login-shell start (under 1 s, 5 s at most) and a look in known places, the first time a recipe runs uv; then nothing.";

    search_max_results: u32 = Value::UInt(5000);
    key "search.max_results";
    kind Kind::UInt { min: 100, max: 1_000_000 };
    class OnDemand;
    lightweight false;
    desc "Cap on project search results";
    cost "Lower is lighter on memory for very common terms.";

    picker_preview: bool = Value::Bool(true);
    key "picker.preview";
    kind Kind::Bool;
    class PerPointer;
    lightweight true;
    desc "File preview in Go to file (Right arrow opens it) and in full-screen search";
    cost "While it is open: one background read of the file under the selection (the usual 2 MB source limit) once the selection rests 150 ms, and syntax colours for the 200 lines shown (about 2 ms in a release build for 200 lines of Rust or Python). No folds and no index.";

    editor_fold_bodies: bool = Value::Bool(true);
    key "editor.fold_bodies";
    kind Kind::Bool;
    class Negligible;
    lightweight true;
    desc "Function bodies folded by default";
    cost "Negligible; a reading preference.";

    editor_json_folds: bool = Value::Bool(true);
    key "editor.json_folds";
    kind Kind::Bool;
    class OnDemand;
    lightweight true;
    desc "Fold JSON objects and arrays";
    cost "One extra parse per JSON file on open, in the background: about 70 ms per 1 MB in a release build. Files up to 256 KB fold before they show; larger ones show at once and get their folds when the parse ends.";

    editor_minimap: bool = Value::Bool(true);
    key "editor.minimap";
    kind Kind::Bool;
    class PerFrame;
    lightweight true;
    desc "Minimap at the right edge of a code pane maximised on its own, or of two wide code panes on a focus page";
    cost "Builds the file's miniature once per file, fold change and comparison (about 3 ms for 10,000 lines in a release build), then paints only the rows in the strip each frame the pane draws: under 1 ms, whatever the file's length or the number of highlights.";

    editor_minimap_always: bool = Value::Bool(false);
    key "editor.minimap_always";
    kind Kind::Bool;
    class Negligible;
    lightweight false;
    desc "Show the minimap in every code pane at least 640 px wide, not only a maximised one";
    cost "Widens where editor.minimap shows (its cost per pane); nothing of its own. Off by default, since the strip takes 100 px.";

    editor_overview: bool = Value::Bool(true);
    key "editor.overview";
    kind Kind::Bool;
    class PerFrame;
    lightweight true;
    desc "Overview lane at the right edge of a code pane with highlights or a comparison: where they are in the whole file";
    cost "Places the marks once per change of highlights, folds or comparison (about 1 ms for 10,000 occurrences), then paints touching marks as one each frame: under 0.2 ms, however many highlights there are.";
    blame_enabled: bool = Value::Bool(true);
    key "blame.enabled";
    kind Kind::Bool;
    class OnDemand;
    lightweight true;
    desc "Toggle Blame command: who last changed each line, beside the gutter";
    cost "Nothing until Toggle Blame is on; then one git blame per file and revision shown, in the background, cached (plus a git rev-parse per pane on window activation): under 10 ms for a small file, under 100 ms for a 2,000-line file with 100+ commits (measured: 70 to 81 ms for a 2,276-line file with 132 commits); seconds on a very long, deep history.";

    scroll_overscroll: bool = Value::Bool(true);
    key "scroll.overscroll";
    kind Kind::Bool;
    class PerFrame;
    lightweight true;
    desc "Lists stretch and spring back when a trackpad scroll runs past an end";
    cost "A little arithmetic per trackpad scroll event in a list, and a redraw each frame for about half a second while a stretched list springs back.";

    graph_enabled: bool = Value::Bool(true);
    key "graph.enabled";
    kind Kind::Bool;
    class OnDemand;
    lightweight true;
    desc "Show graph command";
    cost "Nothing until used; builds the graph in the background on request.";

    debug_frame_overlay: bool = Value::Bool(false);
    key "debug.frame_overlay";
    kind Kind::Bool;
    class PerFrame;
    lightweight true;
    desc "Frame-time overlay";
    cost "Draws a small overlay every frame. Off by default.";

    debug_frame_log: bool = Value::Bool(true);
    key "debug.frame_log";
    kind Kind::Bool;
    class PerFrame;
    lightweight true;
    desc "Write one line per frame to the file named by CHORONG_FRAME_LOG (does nothing while that is unset)";
    cost "A formatted line per frame sent to a writer thread, only while CHORONG_FRAME_LOG is set. Small; a disk write every 200 ms.";

    layout_autosave: bool = Value::Bool(true);
    key "layout.autosave";
    kind Kind::Bool;
    class Negligible;
    lightweight true;
    desc "Remember the pane layout for each repository";
    cost "A copy of the layout and a small file write (off the UI thread, debounced) after each layout change. Nothing while the layout is unchanged.";

    appearance_code_font: String = Value::Text(Cow::Borrowed(cho_fonts_defaults::CODE));
    key "appearance.code_font";
    kind Kind::Font { mono: true };
    class Negligible;
    lightweight false;
    desc "Font for code: the source view, previews and other code text";
    cost "None: text is drawn either way. A font that is not installed falls back to the bundled IBM Plex Mono.";

    appearance_ui_font: String = Value::Text(Cow::Borrowed(cho_fonts_defaults::UI));
    key "appearance.ui_font";
    kind Kind::Font { mono: false };
    class Negligible;
    lightweight false;
    desc "Font for the interface: panes, headers, the status bar and menus";
    cost "None: text is drawn either way. A font that is not installed falls back to the bundled IBM Plex Sans.";
}

impl Default for Settings {
    fn default() -> Self {
        let mut s = Settings {
            appearance_code_font: String::new(),
            appearance_ui_font: String::new(),
            lightweight: false,
            changes_auto_refresh: false,
            context_follow_hover: false,
            context_hover_delay_ms: 0,
            context_follow_caret: false,
            context_references: false,
            hover_card: false,
            hover_peek: false,
            highlight_exact: false,
            highlight_gutter_marks: false,
            lens_call_kinds: false,
            index_auto_refresh: false,
            index_low_priority: false,
            index_banner: false,
            investigations_watch: false,
            index_parallel_projects: 0,
            index_node_memory_mb: 0,
            index_typescript_version: String::new(),
            index_python_version: String::new(),
            index_version_check: false,
            index_version_check_days: 0,
            index_typescript_install: String::new(),
            index_python_install: String::new(),
            tools_node: String::new(),
            tools_npm: String::new(),
            tools_uv: String::new(),
            compare_line_highlight: String::new(),
            compare_changed_words: false,
            compare_follow_highlight: String::new(),
            search_max_results: 0,
            picker_preview: false,
            editor_fold_bodies: false,
            editor_json_folds: false,
            editor_minimap: false,
            editor_minimap_always: false,
            editor_overview: false,
            blame_enabled: false,
            scroll_overscroll: false,
            graph_enabled: false,
            debug_frame_overlay: false,
            debug_frame_log: false,
            layout_autosave: false,
        };
        for d in DEFS {
            s.set(d.key, d.default.clone());
        }
        s
    }
}

/// The bundled fonts' family names, the font settings' defaults.
pub mod cho_fonts_defaults {
    pub const UI: &str = "IBM Plex Sans";
    pub const CODE: &str = "IBM Plex Mono";
}

/// Keys Chorong no longer reads, with what replaced them. A settings file that still has one is
/// told so (kept in the file, ignored), rather than "unknown setting".
pub const RETIRED: &[(&str, &str)] = &[
    (
        "index.auto_build",
        "indexing starts only when you click Build on the banner",
    ),
    (
        "prepare.auto",
        "prepare and indexing start only when you click; `cho open --prepare` on the command line",
    ),
];

/// Why `key` was retired, if it was.
pub fn retired(key: &str) -> Option<&'static str> {
    RETIRED.iter().find(|(k, _)| *k == key).map(|(_, why)| *why)
}

/// All settings, in display order.
pub fn defs() -> &'static [SettingDef] {
    DEFS
}

/// Look a setting up by its dotted key.
pub fn def(key: &str) -> Option<&'static SettingDef> {
    DEFS.iter().find(|d| d.key == key)
}

pub(crate) fn index_of(key: &str) -> Option<usize> {
    DEFS.iter().position(|d| d.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs/public/settings.md: an optional feature that works on every pointer move, every
    /// frame or in the background turns off in Lightweight mode. The rule by cost class, and
    /// a literal list so that changing a setting's class and flag together still shows here.
    #[test]
    fn lightweight_turns_off_every_costly_toggle() {
        for d in DEFS {
            let costly = matches!(
                d.cost_class,
                CostClass::PerFrame | CostClass::PerPointer | CostClass::BackgroundHeavy
            );
            if matches!(d.kind, Kind::Bool | Kind::Choice { .. }) && costly {
                assert!(
                    d.lightweight_off,
                    "{} must turn off in Lightweight mode",
                    d.key
                );
            }
        }
        let mut off: Vec<_> = DEFS
            .iter()
            .filter(|d| d.lightweight_off)
            .map(|d| d.key)
            .collect();
        off.sort_unstable();
        assert_eq!(
            off,
            [
                "blame.enabled",
                "changes.auto_refresh",
                "compare.changed_words",
                "compare.follow_highlight",
                "compare.line_highlight",
                "context.follow_caret",
                "context.follow_hover",
                "context.references",
                "debug.frame_log",
                "debug.frame_overlay",
                "editor.fold_bodies",
                "editor.json_folds",
                "editor.minimap",
                "editor.overview",
                "graph.enabled",
                "highlight.exact",
                "hover.card",
                "hover.peek",
                "index.auto_refresh",
                "index.version_check",
                "investigations.watch",
                "layout.autosave",
                "lens.call_kinds",
                "picker.preview",
                "scroll.overscroll",
            ]
        );
    }

    #[test]
    fn keys_are_unique_and_defaults_valid() {
        for (i, d) in DEFS.iter().enumerate() {
            assert!(
                DEFS[i + 1..].iter().all(|o| o.key != d.key),
                "duplicate {}",
                d.key
            );
            assert!(
                d.kind.check(&d.default).is_ok(),
                "default of {} out of range",
                d.key
            );
            assert!(!d.description.is_empty() && !d.cost_hint.is_empty());
            if d.lightweight_off {
                assert!(
                    matches!(d.kind, Kind::Bool | Kind::Choice { .. }),
                    "{}: only toggles and choices can be turned off",
                    d.key
                );
            }
        }
    }

    #[test]
    fn settings_struct_agrees_with_table() {
        let s = Settings::default();
        for d in DEFS {
            assert_eq!(s.get(d.key), Some(d.default.clone()), "{}", d.key);
        }
        assert_eq!(s.get("nope"), None);
        let mut s = s;
        assert!(s.set("hover.card", Value::Bool(false)) && !s.hover_card);
        assert!(!s.set("hover.card", Value::UInt(1)), "wrong type refused");
        assert!(!s.set("nope", Value::Bool(true)));
    }

    #[test]
    fn documented_defaults_match_the_table() {
        // A row that names its default, "(default 8,192)", names the one in DEFS.
        let doc = include_str!("../../../docs/public/settings.md");
        let mut checked = 0;
        for line in doc.lines() {
            let Some(key) = line.strip_prefix("| `").and_then(|l| l.split('`').next()) else {
                continue;
            };
            let Some(d) = def(key) else { continue };
            let Some(said) = line
                .split("(default ")
                .nth(1)
                .and_then(|r| r.split(')').next())
            else {
                continue;
            };
            assert_eq!(
                said.replace(',', ""),
                d.default.to_string(),
                "docs/public/settings.md gives {key} a different default"
            );
            checked += 1;
        }
        assert!(checked >= 5, "only {checked} documented defaults found");
    }

    #[test]
    fn docs_table_lists_exactly_these_keys() {
        let doc = include_str!("../../../docs/public/settings.md");
        let documented: Vec<&str> = doc
            .lines()
            .filter_map(|l| l.strip_prefix("| `"))
            .filter_map(|l| l.split('`').next())
            .filter(|k| k.contains('.') && !k.ends_with(".json"))
            .collect();
        for d in DEFS.iter().filter(|d| d.key != "lightweight") {
            assert!(
                documented.contains(&d.key),
                "{} is missing from docs/public/settings.md",
                d.key
            );
        }
        for k in documented {
            assert!(
                def(k).is_some(),
                "docs/public/settings.md lists unknown key {k}"
            );
        }
    }

    #[test]
    fn kind_parse() {
        let k = Kind::UInt { min: 1, max: 4 };
        assert_eq!(k.parse("3"), Ok(Value::UInt(3)));
        assert!(k.parse("0").is_err() && k.parse("5").is_err() && k.parse("x").is_err());
        assert!(k.parse("-1").is_err());
        assert_eq!(Kind::Bool.parse("false"), Ok(Value::Bool(false)));
        assert!(Kind::Bool.parse("yes").is_err());
        let c = Kind::Choice {
            options: &["off", "subtle", "stronger"],
        };
        assert_eq!(c.parse("subtle"), Ok(Value::text("subtle")));
        assert!(c.parse("loud").is_err() && c.check(&Value::Bool(true)).is_err());
        assert_eq!(
            def("compare.line_highlight").unwrap().off_value(),
            Value::text("off")
        );
        let f = Kind::Font { mono: true };
        assert_eq!(f.parse(" Menlo "), Ok(Value::text("Menlo")));
        assert!(f.parse("  ").is_err() && f.check(&Value::Bool(true)).is_err());
    }

    #[test]
    fn text_literals_are_json_strings() {
        assert_eq!(Value::text("IBM Plex Mono").literal(), "\"IBM Plex Mono\"");
        assert_eq!(Value::text("a\"b\\c").literal(), "\"a\\\"b\\\\c\"");
    }
}
