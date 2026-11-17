//! The preview ("peek") of a file: Go to file…'s highlighted file (→ opens it, ← closes it),
//! and full-screen search's selected result. Cheap on purpose (`picker.preview`): the file is
//! read in the background through the repository read limits, and only the lines shown are
//! coloured (about 2 ms for 200 lines in a release build). No folds and no index. A leading
//! run of imports is one row. A binary file says so; an image is drawn.

use super::*;
use gpui::Task;
use std::ops::Range;
use std::path::Path;
use std::time::Duration;

/// How long the selection must rest, with the preview open, before the next file is read.
pub(super) const PEEK_DELAY: Duration = Duration::from_millis(150);
/// The most lines drawn.
const PEEK_LINES: usize = 200;
/// Lines shown above a search result's line.
const PEEK_CONTEXT: usize = 12;
/// Characters kept of one line.
const PEEK_LINE_CHARS: usize = 400;
/// Bytes looked at for a NUL, to call a file binary.
const BINARY_SNIFF: usize = 8 * 1024;
/// The largest image drawn.
const IMAGE_MAX_BYTES: u64 = 20 * 1024 * 1024;
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];
/// The fewest import lines folded into one row.
const MIN_FOLDED_IMPORTS: usize = 3;

/// What to preview: a repository-relative file, and the one-based line to show near the top
/// (a search result's), if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PeekKey {
    pub rel: PathBuf,
    pub line: Option<usize>,
}

/// One row of a text preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PeekRow {
    /// Line `n` (one-based), tabs expanded, with its syntax spans (theme keys).
    Line {
        n: usize,
        text: String,
        spans: Vec<(Range<usize>, &'static str)>,
    },
    /// A run of this many import lines, folded.
    Imports(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PeekBody {
    /// `mark` is the one-based line to tint; `total` the file's line count.
    Text {
        rows: Vec<PeekRow>,
        mark: Option<usize>,
        total: usize,
    },
    Image(PathBuf),
    Binary,
    Failed(String),
}

#[derive(Default)]
pub(super) struct PeekState {
    /// What the selection asks for now.
    pub key: Option<PeekKey>,
    /// What was read for `key`; `None` while waiting or reading.
    pub body: Option<PeekBody>,
    /// The next read starts at once (the preview was just opened), not after [`PEEK_DELAY`].
    pub immediate: bool,
    task: Option<Task<()>>,
}

/// Reads `key` under `root` (off the UI thread).
pub(super) fn load(root: &Path, key: &PeekKey) -> PeekBody {
    let ext = key
        .rel
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        // Drawn by GPUI from the path: only a regular file inside the repository, not too big.
        let full = root.join(&key.rel);
        let inside = match (root.canonicalize(), full.canonicalize()) {
            (Ok(r), Ok(f)) if f.starts_with(&r) => Some(f),
            _ => None,
        };
        return match inside.and_then(|f| Some((f.metadata().ok()?, f))) {
            Some((m, f)) if m.is_file() && m.len() <= IMAGE_MAX_BYTES => PeekBody::Image(f),
            Some((m, _)) if m.is_file() => {
                PeekBody::Failed("The image is too large to preview".into())
            }
            _ => PeekBody::Failed("Can't read this file".into()),
        };
    }
    let bytes = match cho_core::fs::read_source_in(root, &key.rel) {
        Ok(b) => b,
        Err(e) => return PeekBody::Failed(e.to_string()),
    };
    if bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0) {
        return PeekBody::Binary;
    }
    let language = cho_syntax::Language::from_path(&key.rel);
    text_body(&String::from_utf8_lossy(&bytes), key.line, language)
}

/// Whether `line` starts an import (`use`, `import`, `from … import`, `#include`, …).
fn starts_import(line: &str) -> bool {
    let t = line.trim_start();
    [
        "use ",
        "pub use ",
        "pub(crate) use ",
        "import ",
        "from ",
        "#include",
        "extern crate ",
        "@import ",
    ]
    .iter()
    .any(|p| t.starts_with(p))
}

/// The leading run of import lines to fold, as line indices: from the first import (after
/// blank lines, comments and a `package` line) to the last line of the last import, braces and
/// parentheses of a multi-line import included. `None` when there are fewer than
/// [`MIN_FOLDED_IMPORTS`] lines in it.
fn import_run(lines: &[&str]) -> Option<Range<usize>> {
    let lead = |t: &str| {
        t.is_empty()
            || ["//", "#!", "/*", "*", "#", "--", "package ", "\"\"\""]
                .iter()
                .any(|p| t.starts_with(p))
                && !starts_import(t)
    };
    let start = lines
        .iter()
        .position(|l| !lead(l.trim()))
        .filter(|&i| starts_import(lines[i]))?;
    let (mut depth, mut end) = (0i32, start);
    for (i, l) in lines.iter().enumerate().skip(start) {
        let t = l.trim();
        if depth > 0 || starts_import(t) {
            depth += t.matches(['{', '(']).count() as i32;
            depth -= t.matches(['}', ')']).count() as i32;
            end = i + 1;
        } else if !t.is_empty() {
            break;
        }
    }
    (end - start >= MIN_FOLDED_IMPORTS).then_some(start..end)
}

/// Tabs expanded and the line cut at [`PEEK_LINE_CHARS`].
fn shown_line(l: &str) -> String {
    let l = l.replace('\t', "    ");
    match l.char_indices().nth(PEEK_LINE_CHARS) {
        Some((cut, _)) => format!("{}\u{2026}", &l[..cut]),
        None => l,
    }
}

/// Rows for the lines `range` (indices) of `lines`, coloured together so a statement that
/// spans lines parses as one.
fn coloured(
    lines: &[&str],
    range: Range<usize>,
    language: Option<cho_syntax::Language>,
) -> Vec<PeekRow> {
    let shown: Vec<String> = lines[range.clone()].iter().map(|l| shown_line(l)).collect();
    let joined = shown.join("\n");
    let spans = language
        .and_then(|l| cho_syntax::highlight_shared(l, &joined).ok())
        .unwrap_or_default();
    let mut rows = Vec::with_capacity(shown.len());
    let mut at = 0;
    for (i, text) in shown.into_iter().enumerate() {
        let line = at..at + text.len();
        let mine = spans
            .iter()
            .filter(|s| s.range.start < line.end && s.range.end > line.start)
            .map(|s| {
                let a = s.range.start.max(line.start) - line.start;
                let b = s.range.end.min(line.end) - line.start;
                (a..b, s.theme_key())
            })
            .filter(|(r, _)| text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
            .collect();
        at = line.end + 1;
        rows.push(PeekRow::Line {
            n: range.start + i + 1,
            text,
            spans: mine,
        });
    }
    rows
}

/// The rows to show of `text`: from a little above `line`, or from the top with a leading run
/// of imports folded into one row.
fn text_body(text: &str, line: Option<usize>, language: Option<cho_syntax::Language>) -> PeekBody {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let window = |from: usize| from.min(total)..(from + PEEK_LINES).min(total);
    let rows = match (line, import_run(&lines)) {
        (Some(l), _) => coloured(
            &lines,
            window(l.saturating_sub(1).saturating_sub(PEEK_CONTEXT)),
            language,
        ),
        (None, Some(run)) => {
            let mut rows = coloured(&lines, 0..run.start, language);
            rows.push(PeekRow::Imports(run.len()));
            rows.extend(coloured(&lines, window(run.end), language));
            rows
        }
        (None, None) => coloured(&lines, window(0), language),
    };
    PeekBody::Text {
        rows,
        mark: line,
        total,
    }
}

impl Chorong {
    /// What the preview should show now: Go to file…'s highlighted file while its preview is
    /// open, or the selected result while search fills the window. `None` when neither, or the
    /// setting is off.
    fn peek_wanted(&self) -> Option<PeekKey> {
        if !self.features.on(Feature::PickerPreview) {
            return None;
        }
        if let Some(p) = &self.picker {
            if p.kind != PickerKind::Files || !p.peek_open {
                return None;
            }
            let m = p.matches.get(p.selected)?;
            return Some(PeekKey {
                rel: self.files.get(m.index)?.clone(),
                line: None,
            });
        }
        if self.search_full_screen() {
            let l = self.search.as_ref()?.selected_line()?;
            return Some(PeekKey {
                rel: PathBuf::from(&l.path),
                line: Some(l.line),
            });
        }
        None
    }

    /// → and ← in Go to file…: open or close the preview beside the list.
    pub(super) fn set_picker_peek(&mut self, open: bool) {
        if !self.features.on(Feature::PickerPreview) {
            return;
        }
        if let Some(p) = &mut self.picker
            && p.kind == PickerKind::Files
            && p.peek_open != open
        {
            p.peek_open = open;
            self.peek.immediate = open;
        }
    }

    /// Once per frame: when the wanted preview changed, forget the old one and read the new
    /// one (at once when the preview was just opened, else once the selection rests).
    pub(super) fn sync_peek(&mut self, cx: &mut Context<Self>) {
        let want = self.peek_wanted();
        if want == self.peek.key {
            return;
        }
        self.peek.key = want.clone();
        self.peek.body = None;
        self.peek.task = None;
        let delay = if std::mem::take(&mut self.peek.immediate) {
            Duration::ZERO
        } else {
            PEEK_DELAY
        };
        let (Some(key), Some(root)) = (want, self.root.clone()) else {
            return;
        };
        let cost = self.features.probe(Feature::PickerPreview);
        self.peek.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let k = key.clone();
            let body = cx
                .background_spawn(async move {
                    let _t = cost.background();
                    load(&root, &k)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.peek.key.as_ref() == Some(&key) {
                    this.peek.body = Some(body);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// The preview panel: the file's name, then its lines, the image, or why there is none.
    pub(super) fn render_peek(&self) -> Option<gpui::AnyElement> {
        let key = self.peek.key.as_ref()?;
        let t = &self.theme;
        let muted = hsla(t.text_muted);
        let note = |text: String| {
            div()
                .p_3()
                .text_size(px(12.5))
                .text_color(muted)
                .child(text)
                .into_any_element()
        };
        let body = match &self.peek.body {
            // Reading (or waiting for the selection to rest).
            None => div().into_any_element(),
            Some(PeekBody::Binary) => note("Can't preview a binary file".into()),
            Some(PeekBody::Failed(why)) => note(why.clone()),
            Some(PeekBody::Image(path)) => div()
                .flex_1()
                .min_h_0()
                .p_3()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    gpui::img(path.clone())
                        .max_w_full()
                        .max_h_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .into_any_element(),
            Some(PeekBody::Text { rows, mark, total }) => {
                let width = total.max(&1).to_string().len();
                let tint = hsla(
                    t.players
                        .first()
                        .map_or(t.element_selected, |p| p.selection),
                );
                let number = |n: Option<usize>| {
                    div().flex_none().pr_3().text_color(muted).child(match n {
                        Some(n) => format!("{n:>width$}"),
                        None => " ".repeat(width),
                    })
                };
                self.scroll_areas
                    .get("peek-lines")
                    .vertical(
                        "peek-lines",
                        self.features.probe_if_on(Feature::Overscroll),
                        div()
                            .py_2()
                            .font_family(self.font.clone())
                            .text_size(px(12.))
                            .children(rows.iter().map(|row| {
                                match row {
                                    PeekRow::Line { n, text, spans } => {
                                        let styles: Vec<_> = spans
                                            .iter()
                                            .filter_map(|(r, k)| {
                                                Some((r.clone(), crate::document::key_style(k, t)?))
                                            })
                                            .collect();
                                        div()
                                            .flex()
                                            .whitespace_nowrap()
                                            .overflow_hidden()
                                            .px_2()
                                            .when(Some(*n) == *mark, |d| d.bg(tint))
                                            .child(number(Some(*n)))
                                            .child(
                                                div().text_color(hsla(t.text)).child(
                                                    StyledText::new(text.clone())
                                                        .with_highlights(styles),
                                                ),
                                            )
                                    }
                                    PeekRow::Imports(count) => div()
                                        .flex()
                                        .px_2()
                                        .text_color(muted)
                                        .child(number(None))
                                        .child(format!(
                                            "\u{22ef} {count} import line{}",
                                            if *count == 1 { "" } else { "s" }
                                        )),
                                }
                            })),
                    )
                    .flex_1()
                    .min_h_0()
                    .into_any_element()
            }
        };
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(hsla(t.border))
                .bg(hsla(t.editor_background))
                .child(
                    div()
                        .flex_none()
                        .h(px(28.))
                        .px_3()
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(hsla(t.border))
                        .text_size(px(12.))
                        .text_color(muted)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(key.rel.display().to_string()),
                )
                .child(body)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(b: &PeekBody) -> Vec<Option<usize>> {
        let PeekBody::Text { rows, .. } = b else {
            panic!("text")
        };
        rows.iter()
            .map(|r| match r {
                PeekRow::Line { n, .. } => Some(*n),
                PeekRow::Imports(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_search_line_shows_a_little_above_it_and_long_lines_are_cut() {
        let text: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        let b = text_body(&text, Some(50), None);
        assert_eq!(numbers(&b)[0], Some(38));
        let long = "x".repeat(1000);
        let PeekBody::Text { rows, .. } = text_body(&long, None, None) else {
            panic!("text")
        };
        let PeekRow::Line { text, .. } = &rows[0] else {
            panic!("line")
        };
        assert_eq!(text.chars().count(), PEEK_LINE_CHARS + 1);
    }

    #[test]
    fn leading_imports_fold_into_one_row_and_doc_comments_stay() {
        let rust =
            "//! Module docs.\n\nuse a::b;\nuse c::{\n    d,\n    e,\n};\nuse f;\n\nfn g() {}\n";
        let b = text_body(rust, None, None);
        // Lines 1-2 shown, imports 3-8 folded, then line 9 on.
        assert_eq!(numbers(&b), [Some(1), Some(2), None, Some(9), Some(10)]);
        let PeekBody::Text { rows, .. } = &b else {
            panic!("text")
        };
        assert_eq!(rows[2], PeekRow::Imports(6));
        let ts = "import x from 'x';\nimport {\n  y,\n} from 'y';\n\nexport const z = 1;\n";
        assert_eq!(numbers(&text_body(ts, None, None))[..2], [None, Some(5)]);
        // Two imports are not worth a fold; code that does not start with imports is untouched.
        assert!(import_run(&["use a;", "use b;", "fn c() {}"]).is_none());
        assert!(import_run(&["fn main() {}", "use a;", "use b;", "use c;"]).is_none());
    }

    #[test]
    fn shown_lines_are_coloured_by_the_file_language() {
        let PeekBody::Text { rows, .. } =
            text_body("fn main() {}\n", None, Some(cho_syntax::Language::Rust))
        else {
            panic!("text")
        };
        let PeekRow::Line { spans, .. } = &rows[0] else {
            panic!("line")
        };
        assert!(spans.iter().any(|(r, _)| *r == (0..2)), "{spans:?}");
    }

    #[test]
    fn binary_files_and_images_are_told_apart_and_reads_stay_in_the_repository() {
        let dir = std::env::temp_dir().join(format!("cho-peek-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin"), [1u8, 0, 2]).unwrap();
        std::fs::write(dir.join("a.png"), [0u8; 4]).unwrap();
        std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();
        let key = |p: &str| PeekKey {
            rel: PathBuf::from(p),
            line: None,
        };
        assert_eq!(load(&dir, &key("a.bin")), PeekBody::Binary);
        assert!(matches!(load(&dir, &key("a.png")), PeekBody::Image(_)));
        assert!(matches!(
            load(&dir, &key("a.rs")),
            PeekBody::Text { total: 1, .. }
        ));
        assert!(matches!(load(&dir, &key("../x.png")), PeekBody::Failed(_)));
        std::fs::remove_dir_all(&dir).ok();
    }
}
