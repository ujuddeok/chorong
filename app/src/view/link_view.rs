//! The pull-request parts of `chorong://` links (provisional, `docs/public/url-scheme.md`):
//! the question before a link fetches a pull request, the Before a link's comparison starts
//! with, and the side and place it puts the reader on.
//!
//! Nothing here fetches: the question's answer starts the same open as "Open Remote…"
//! (`Chorong::begin_link_open`), and a link whose pull request is already in the cache only
//! reads what is there.

use super::*;
use crate::AskPr;
use crate::remote::adapter;
use cho_core::LinkSide;

/// A link's side and place, waiting for the file and its other side to be ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingLink {
    path: PathBuf,
    symbol: Option<String>,
    /// One-based.
    line: Option<usize>,
}

/// The Before a link's comparison starts with, found in the background.
struct Before {
    base: String,
    against: String,
    merge_bases: usize,
    /// Said before the comparison notice ("That Before commit isn't available.").
    prefix: Option<String>,
}

/// The link's `base` if this repository has it, else the merge base with the pull request's
/// target (or the default branch), saying so when the link named one that is missing.
fn resolve_before(root: &std::path::Path, base: Option<&str>) -> Result<Before, String> {
    let found = base.and_then(|b| cho_git::rev_parse(root, b));
    match links::choose_before(base, found) {
        links::BeforeChoice::Link(full) => Ok(Before {
            against: cho_git::short(&full),
            base: full,
            merge_bases: 0,
            prefix: None,
        }),
        links::BeforeChoice::MergeBase { missing } => {
            let prefix = missing.then(|| links::MISSING_BASE.to_string());
            let with = |why: String| match &prefix {
                Some(p) => format!("{p} {why}"),
                None => why,
            };
            let target = adapter::read_header(root)
                .and_then(|h| h.target)
                .or_else(|| cho_git::preferred_refs(root).default_branch)
                .ok_or_else(|| with("There is no branch to compare with.".into()))?;
            let (_, base, merge_bases) = compare_git::merge_base(root, &target).map_err(with)?;
            Ok(Before {
                base,
                against: target,
                merge_bases,
                prefix,
            })
        }
    }
}

impl Chorong {
    /// A pull-request link and nothing cached for it: ask before fetching.
    pub fn ask_open_pull_request(&mut self, ask: AskPr, cx: &mut Context<Self>) {
        self.picker = None;
        self.overlay = Some(Overlay::OpenPullRequest(ask));
        cx.notify();
    }

    /// The question: it names the pull request and says what saying yes does.
    pub(super) fn render_open_pull_request(
        &self,
        ask: &AskPr,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let muted = hsla(t.text_muted);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(hsla(t.border))
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .when(primary, |d| d.bg(hsla(t.element_selected)))
                .hover(|s| s.bg(hsla(t.element_selected)))
                .flex()
                .items_center()
                .gap_2()
                .child(label)
                .children(self.dialog_key(primary))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if primary {
                        this.dialog_accept(window, cx);
                    } else {
                        this.overlay = None;
                    }
                    cx.notify();
                }))
        };
        let card = self
            .card(560.)
            .p_4()
            .gap_2()
            .child(div().font_weight(FontWeight::MEDIUM).child(format!(
                "Open pull request #{} of {}?",
                ask.number, ask.repo
            )))
            .child(div().text_color(muted).child(
                "A link from outside Chorong (a web page, a chat, another program) asked for \
                 this. Chorong will fetch the pull request into its cache: a clone of the \
                 repository the first time, then the pull request's commits. Fetching only \
                 reads from the host, and nothing in the repository runs. Open it only if \
                 you expected this link.",
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child(ask.info.url.clone()),
            )
            .children(ask.local.as_ref().map(|folder| {
                div().flex().justify_end().child({
                    let folder = folder.clone();
                    div()
                        .id("open-pr-local")
                        .max_w_full()
                        .overflow_hidden()
                        .px_3()
                        .py_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(hsla(t.border))
                        .cursor_pointer()
                        .hover(|s| s.bg(hsla(t.element_selected)))
                        .child(format!(
                            "Open your worktree at this commit ({})",
                            links::tilde(&folder)
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_local_choice(&folder, cx);
                        }))
                })
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(button("open-pr-cancel", "Cancel", false))
                    .child(button("open-pr-accept", "Open", true)),
            );
        self.layer(card, cx)
    }

    /// The second choice of the question: the same link in the reader's own worktree at the
    /// linked commit. Fetches nothing; the link then opens like any `rev` + `base` link.
    fn open_local_choice(&mut self, folder: &std::path::Path, cx: &mut Context<Self>) {
        let Some(Overlay::OpenPullRequest(ask)) = self.overlay.take() else {
            return;
        };
        match links::local_link(&ask.info, folder) {
            Some(url) => crate::send_link(url, cx),
            None => self.notify_text("Cannot open that worktree", cx),
        }
        cx.notify();
    }

    /// A link with a `base` or a `pr` opened in this window (its pull request already in the
    /// cache, or a local worktree): starts the comparison with the link's Before, or with the
    /// merge base when the link has none or Chorong does not have it (with a notice).
    pub(super) fn start_link_comparison(&mut self, info: &LinkInfo, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let base = info.base.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { resolve_before(&root, base.as_deref()) })
                .await;
            this.update(cx, |this, cx| match found {
                Ok(b) => this.start_comparison(b.base, b.against, b.merge_bases, b.prefix, cx),
                Err(e) => this.notify_text(e, cx),
            })
            .ok();
        })
        .detach();
    }

    /// A link made on Before: once `file` is open and its other side is built, the active
    /// pane goes to Before at the link's declaration (or line).
    pub(super) fn expect_link_side(&mut self, info: &LinkInfo, file: PathBuf) {
        self.pending_link = (info.side == Some(LinkSide::Before)).then(|| PendingLink {
            path: file,
            symbol: info.symbol.clone(),
            line: info.line,
        });
    }

    /// A window already in Split on the link's file: the column of the link's side takes
    /// focus, at the link's place. False when there is no such split (the link then opens
    /// the file the usual way, which ends a split).
    pub(super) fn focus_split_side(&mut self, info: &LinkInfo, cx: &mut Context<Self>) -> bool {
        let (Some(link), Some(side)) = (
            self.links
                .iter()
                .copied()
                .find(|l| l.has(self.active_editor)),
            info.side,
        ) else {
            return false;
        };
        let pane = match side {
            LinkSide::Before => link.before,
            LinkSide::After => link.after,
        };
        let same_file = info.path.as_deref().is_some_and(|p| {
            self.ed_of(pane)
                .doc
                .as_ref()
                .is_some_and(|d| d.path.ends_with(p))
        });
        if !same_file || !self.editors.contains_key(&pane) {
            return false;
        }
        self.shell.update(cx, |shell, cx| {
            let _ = shell.update_workspace(cx, |ws| ws.activate(pane));
        });
        self.active_editor = pane;
        let target = Target::at(info.symbol.clone(), info.line);
        self.with_pane(pane, |this| this.apply_target(&target, cx));
        self.dirty_all = true;
        true
    }

    /// `pane`'s other side has been built (or turned out not to exist): if a link is waiting
    /// on this file, switch to its side and go to its place.
    pub(super) fn apply_pending_link(&mut self, pane: PaneId, built: bool, cx: &mut Context<Self>) {
        let Some(p) = self.pending_link.as_ref() else {
            return;
        };
        let here = self.ed_of(pane).doc.as_ref().map(|d| d.path.clone());
        if pane != self.active_editor || here.as_deref() != Some(p.path.as_path()) {
            return;
        }
        let Some(p) = self.pending_link.take() else {
            return;
        };
        if !built {
            self.notify_text("This file is new in this change; showing After", cx);
            return;
        }
        if self.ed_of(pane).side == Side::Before {
            return;
        }
        self.switch_side(cx);
        let target = Target::at(p.symbol, p.line);
        self.with_pane(pane, |this| this.apply_target(&target, cx));
    }

    /// What the links copied from this window add: the pull request (a window on a cache
    /// worktree opened for one), its head, the comparison's Before and the active pane's
    /// side. `None` for a window that is neither a pull-request view nor comparing.
    pub(super) fn link_view_params(&self) -> Option<links::ViewParams> {
        let header = self.remote_header.as_ref().map(|h| &h.header);
        let side = match self.ed().side {
            Side::Before => LinkSide::Before,
            Side::After => LinkSide::After,
        };
        let head = header.map(|h| h.head.as_str());
        links::ViewParams::of(
            header.map(|h| h.repo.clone()),
            header.and_then(|h| h.pull_request()),
            head,
            self.compare.as_ref().map(|c| c.base.as_str()),
            side,
        )
    }
}
