//! The search panel: running searches off the UI thread and drawing results.

use super::shell_host::OpenAt;
use super::*;
use crate::search::SearchStatus;

impl Chorong {
    // ---- Repository search ----

    pub(super) fn open_search(&mut self, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return;
        }
        // Again while search has the keyboard (a quick double tap included): grow it to the
        // window, or back.
        if self.search.is_some() && self.key_focus == KeyFocus::Search {
            self.toggle_search_full(cx);
            return;
        }
        self.picker = None;
        self.search.get_or_insert_with(SearchState::default);
        self.focus_to(KeyFocus::Search);
        cx.notify();
    }

    /// Whether search fills the window (grown with Cmd-Shift-F). Never reads the shell: the
    /// search pane's body calls it while the shell is drawing, and reading the shell there is a
    /// GPUI double lease (a crash). `Chorong::render` clears the flag when the focus layout it
    /// opened has gone ([`Self::settle_search_full`]).
    pub(super) fn search_full_screen(&self) -> bool {
        self.search_full && self.search.is_some()
    }

    /// Before the shell draws: full-screen search ends when its focus layout was closed some
    /// other way (Esc, Exit focus).
    pub(super) fn settle_search_full(&mut self, cx: &App) {
        if self.search_full && !self.shell.read(cx).workspace().in_focus() {
            self.search_full = false;
        }
    }

    /// Cmd-Shift-F with search open: the search pane (the file tree's) maximised on its own,
    /// or the layout restored. Inside a focus layout of your own it says so and does nothing.
    fn toggle_search_full(&mut self, cx: &mut Context<Self>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if self.search_full_screen() {
            self.search_full = false;
            self.shell.update(cx, |shell, cx| {
                let _ = shell.update_workspace(cx, |ws| ws.restore(now).map(|_| ()));
            });
            cx.notify();
            return;
        }
        let ws = self.shell.read(cx).workspace();
        if ws.in_focus() {
            self.notify_text(
                "Search fills the window from the normal layout: leave focus first",
                cx,
            );
            return;
        }
        let Some(pane) = super::shell_host::pane_of(ws, cho_layout::PaneKind::FileTree) else {
            return;
        };
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| ws.maximize(pane, now));
        });
        self.search_full = true;
        self.focus_to(KeyFocus::Search);
        cx.notify();
    }

    pub(super) fn close_search(&mut self, cx: &mut Context<Self>) {
        if self.search_full_screen() {
            // Closing full-screen search puts the layout back as it was.
            self.toggle_search_full(cx);
        }
        self.search_full = false;
        if let Some(s) = self.search.take() {
            s.cancel.cancel();
        }
        self.key_focus.leave(KeyFocus::Search);
        cx.notify();
    }

    /// Starts a search for the current query after the debounce. Any running search is
    /// cancelled, and a result for an older query is dropped by the generation check.
    pub(super) fn schedule_search(&mut self, cx: &mut Context<Self>) {
        // "Changed files only" narrows the search to the Changes list's files.
        let changed = self
            .search
            .as_ref()
            .filter(|s| s.changed_only)
            .map(|_| self.changed_file_globs());
        let (Some(s), Some(root)) = (&mut self.search, self.root.clone()) else {
            return;
        };
        s.cancel.cancel();
        s.cancel = CancelToken::new();
        let generation = s.generation.next();
        let cancel = s.cancel.clone();
        if s.query.is_empty() {
            s.clear_results();
            return;
        }
        let globs = match changed {
            None => Vec::new(),
            Some(Ok(globs)) if !globs.is_empty() => globs,
            Some(Ok(_)) => {
                s.fail("No changed files in this folder".into());
                return;
            }
            Some(Err(why)) => {
                s.fail(why);
                return;
            }
        };
        s.start();
        let opts = SearchOptions {
            pattern: s.query.clone(),
            regex: s.regex,
            case_sensitive: s.case_sensitive,
            globs,
            max_results: self.features.settings().search_max_results as usize,
        };
        let cost = self.features.probe(Feature::SearchMaxResults);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let current = this
                .update(cx, |this, _| {
                    this.search
                        .as_ref()
                        .is_some_and(|s| s.generation.is(generation))
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let result = cx
                .background_spawn(async move {
                    let _t = cost.background();
                    search_all(&root, &opts, &cancel)
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(s) = this.search.as_mut().filter(|s| s.generation.is(generation)) else {
                    return;
                };
                match result {
                    Ok((hits, summary)) => s.set_results(&hits, summary.truncated),
                    Err(e) => s.fail(e.to_string()),
                }
                this.search_scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn search_step(&mut self, delta: isize) {
        if let Some(s) = &mut self.search {
            s.step(delta);
            if let Some(i) = s.selected {
                let strategy = step_strategy(delta);
                self.search_scroll.scroll_to_item(i, strategy);
            }
        }
    }

    /// Opens a result at its first match and records history.
    pub(super) fn open_search_result(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(s) = &mut self.search else { return };
        s.selected = Some(row);
        if let Some(l) = s.selected_line() {
            let (path, byte) = (l.path.clone(), l.byte);
            self.navigate(&path, byte, cx);
        }
    }

    pub(super) fn render_search(
        &self,
        search: &SearchState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let (muted, border, selected_bg) =
            (hsla(t.text_muted), hsla(t.border), hsla(t.element_selected));
        let focused = self.key_focus == KeyFocus::Search;
        let accent = t
            .players
            .first()
            .map_or(t.element_selected, |p| p.cursor.with_alpha(0x66));
        let toggle = |id: &'static str, label: &'static str, on: bool, tip_action: u8| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .when(on, |d| d.bg(selected_bg).text_color(hsla(t.text)))
                .when(!on, |d| d.text_color(muted))
                .hover(|s| s.bg(selected_bg))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(s) = &mut this.search {
                        match tip_action {
                            0 => s.case_sensitive = !s.case_sensitive,
                            1 => s.regex = !s.regex,
                            _ => s.changed_only = !s.changed_only,
                        }
                        this.focus_to(KeyFocus::Search);
                    }
                    this.schedule_search(cx);
                    cx.notify();
                }))
        };
        let summary = if let SearchStatus::Failed(e) = &search.status {
            e.clone()
        } else if search.query.is_empty() && search.changed_only && self.compare.is_some() {
            "Type to search the files changed in this comparison.".into()
        } else if search.query.is_empty() {
            "Type to search the folder (respects .gitignore).".into()
        } else if search.status == SearchStatus::Running && search.lines.is_empty() {
            "Searching\u{2026}".into()
        } else if search.lines.is_empty() {
            "No results".into()
        } else {
            let more = if search.truncated {
                format!(
                    " (stopped at {})",
                    self.features.settings().search_max_results
                )
            } else {
                String::new()
            };
            format!(
                "{} result{} in {} file{}{more}",
                search.lines.len(),
                if search.lines.len() == 1 { "" } else { "s" },
                search.files,
                if search.files == 1 { "" } else { "s" },
            )
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .child(
                div()
                    .flex_none()
                    .h(px(30.))
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .text_color(muted)
                    .child("Search"),
            )
            .child(
                div()
                    .id("search-input")
                    .flex_none()
                    .m_2()
                    .px_2()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(if focused {
                        hsla(t.players.first().map_or(t.border, |p| p.cursor))
                    } else {
                        border
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.search.is_some() {
                            this.focus_to(KeyFocus::Search);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .when(search.query.is_empty(), |d| {
                                d.child(div().text_color(muted).child("Search files\u{2026}"))
                            })
                            .child(self.query_text(&search.query, search.query_all))
                            .when(focused, |d| {
                                d.child(div().w(px(1.5)).h(px(14.)).bg(hsla(t.text)))
                            }),
                    )
                    .child(toggle("search-case", "Aa", search.case_sensitive, 0))
                    .child(toggle("search-regex", ".*", search.regex, 1))
                    .when(self.compare.is_some(), |d| {
                        d.child(toggle("search-changed", "Changed", search.changed_only, 2))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .pb_1()
                    .text_size(px(11.5))
                    .text_color(muted)
                    .child(summary),
            )
            .child(
                crate::scroll::uniform_list(
                    "search-results",
                    search.rows.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        let Some(s) = &this.search else {
                            return Vec::new();
                        };
                        range
                            .map(|ix| match &s.rows[ix] {
                                SearchRow::File { path, count } => {
                                    let (name, folder) = match path.rsplit_once('/') {
                                        Some((dir, name)) => (name.to_string(), dir.to_string()),
                                        None => (path.clone(), String::new()),
                                    };
                                    div()
                                        .id(("search-file", ix))
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .px_3()
                                        .h(px(24.))
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(name)
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .font_weight(FontWeight::NORMAL)
                                                .text_color(muted)
                                                .child(folder),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .font_weight(FontWeight::NORMAL)
                                                .text_color(muted)
                                                .child(format!("{count}")),
                                        )
                                }
                                SearchRow::Line(li) => {
                                    let l = &s.lines[*li];
                                    let highlights = l
                                        .ranges
                                        .iter()
                                        .map(|r| {
                                            (
                                                r.clone(),
                                                HighlightStyle {
                                                    background_color: Some(hsla(accent)),
                                                    font_weight: Some(FontWeight::BOLD),
                                                    ..Default::default()
                                                },
                                            )
                                        })
                                        .collect::<Vec<_>>();
                                    let selected = s.selected == Some(ix);
                                    div()
                                        .id(("search-line", ix))
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .pl_4()
                                        .pr_2()
                                        .h(px(20.))
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .cursor_pointer()
                                        .when(selected, |d| d.bg(selected_bg))
                                        .hover(|st| st.bg(selected_bg))
                                        .child(
                                            div()
                                                .flex_none()
                                                .min_w(px(26.))
                                                .text_right()
                                                .text_size(px(11.))
                                                .text_color(muted)
                                                .child(l.line.to_string()),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .font_family(this.font.clone())
                                                .text_size(px(11.5))
                                                .child(
                                                    StyledText::new(l.text.clone())
                                                        .with_highlights(highlights),
                                                ),
                                        )
                                        .on_click(cx.listener(
                                            move |this, e: &gpui::ClickEvent, _, cx| {
                                                this.prepare_place(
                                                    OpenAt::of_click(&e.modifiers()),
                                                    cx,
                                                );
                                                this.open_search_result(ix, cx);
                                                cx.notify();
                                            },
                                        ))
                                }
                            })
                            .collect()
                    }),
                )
                .wrap(
                    &self.search_over,
                    &self.search_scroll,
                    self.features.probe_if_on(Feature::Overscroll),
                )
                .flex_1()
                .min_h_0(),
            )
    }
}

// Actions, commands and registration (see `actions.rs`).

actions!(
    chorong,
    [
        OpenSearch,
        SearchPrev,
        SearchNext,
        SearchConfirm,
        SearchClose,
        SearchToggleCase,
        SearchToggleRegex,
    ]
);

pub(super) const COMMANDS: &[Command] = &[c(
    "chorong::OpenSearch",
    "Search the Project",
    Layer::Go,
    110,
)];

pub(super) fn register(d: Div, cx: &mut Context<Chorong>) -> Div {
    let d = on!(d, cx, OpenSearch, |this, _w, cx| this.open_search(cx));
    let d = on!(d, cx, SearchClose, |this, _w, cx| this.close_search(cx));
    let d = on!(d, cx, SearchPrev, |this, _w, cx| this.search_step(-1));
    let d = on!(d, cx, SearchNext, |this, _w, cx| this.search_step(1));
    let d = on!(d, cx, SearchConfirm, |this, _w, cx| {
        if let Some(row) = this.search.as_ref().and_then(|s| s.selected) {
            this.open_search_result(row, cx);
        }
    });
    let d = on!(d, cx, SearchToggleCase, |this, _w, cx| {
        if let Some(s) = &mut this.search {
            s.case_sensitive = !s.case_sensitive;
        }
        this.schedule_search(cx);
    });
    on!(d, cx, SearchToggleRegex, |this, _w, cx| {
        if let Some(s) = &mut this.search {
            s.regex = !s.regex;
        }
        this.schedule_search(cx);
    })
}
