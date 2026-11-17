//! Pointer and keyboard focus: what the context pane follows. Pinning is in `pins.rs`.

use super::*;

/// The regions a hover is kept across.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PointerRegion {
    Source,
    ContextPane,
}

impl Chorong {
    /// Where the context pane looks: the hovered identifier after the dwell, else the caret.
    pub(super) fn context_focus(&self) -> Option<usize> {
        // A hover is followed only in the active editor; a hover left in another pane is stale.
        let hovered = self
            .hover
            .active()
            .filter(|_| self.hover_pane == self.active_editor);
        let caret = self
            .ed()
            .caret
            .filter(|_| self.features.on(Feature::FollowCaret));
        hovered.map(|r| r.start).or(caret)
    }

    /// The pointer moved over the source. `ident` is the identifier under it, if any. After
    /// the dwell the context pane follows it; the query itself runs in the background.
    pub(super) fn hover_moved(
        &mut self,
        pane: PaneId,
        ident: Option<std::ops::Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        // Only the active editor drives the context pane; the pointer over another one is
        // treated as resting on nothing.
        self.hover_pane = pane;
        let ident = ident.filter(|_| pane == self.active_editor);
        if !self.features.on(Feature::FollowHover) {
            return;
        }
        let _t = self.features.time(Feature::FollowHover);
        let Some(generation) = self.hover.moved(ident) else {
            return;
        };
        let delay =
            Duration::from_millis(u64::from(self.features.settings().context_hover_delay_ms));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                if this.features.on(Feature::FollowHover) && this.hover.dwelled(generation) {
                    let cost = this.features.probe(Feature::FollowHover);
                    let _t = cost.start();
                    this.refresh_context(cx);
                    this.dirty_pane = Some(this.active_editor);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The pointer left both the source and the pane: the pane goes back to the caret.
    pub(super) fn hover_left(&mut self, cx: &mut Context<Self>) {
        if self.hover.left() {
            self.refresh_context(cx);
            self.dirty_pane = Some(self.active_editor);
            cx.notify();
        }
    }

    /// The pointer entered (`inside`) or left the source view or the context pane.
    /// Travelling from one to the other keeps the hover; leaving both, after a short grace
    /// for the crossing, returns the pane to the caret.
    pub(super) fn pointer_region(
        &mut self,
        region: PointerRegion,
        inside: bool,
        cx: &mut Context<Self>,
    ) {
        match region {
            PointerRegion::Source => self.pointer_in_source = inside,
            PointerRegion::ContextPane => self.pointer_in_pane = inside,
        }
        // Entering the pane: an identifier crossed on the way never takes over. Leaving the
        // source: nothing under the pointer is pending any more.
        if inside == (region == PointerRegion::ContextPane) {
            self.hover.cancel_pending();
        }
        if inside {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LEAVE_GRACE).await;
            this.update(cx, |this, cx| {
                if !this.pointer_in_source && !this.pointer_in_pane {
                    this.hover_left(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// What the top of the context pane shows for the current focus (hover or caret).
    pub(super) fn pane_focus(&self) -> Option<PaneFocus> {
        let (doc, at) = (self.ed().doc.as_ref()?, self.context_focus()?);
        let decl = enclosing(&doc.symbols, at).map(|sym| {
            let start = doc.buffer.line_of(sym.range.start) + 1;
            let end = doc.buffer.line_of(sym.range.end.saturating_sub(1)) + 1;
            let name = match doc.language {
                Some(l) => sym.qualified_name(l),
                None => sym.name.clone(),
            };
            (sym.kind.as_str().to_string(), name, start, end)
        });
        let ident = identifier_at(&doc.analysis.identifiers, at)
            .and_then(|r| doc.buffer.text().get(r).map(str::to_string));
        Some(PaneFocus { decl, ident })
    }

    /// The (file, identifier start) the pane's index answer is for.
    pub(super) fn focus_key(&self) -> Option<(PathBuf, usize)> {
        let (doc, at) = (self.ed().doc.as_ref()?, self.context_focus()?);
        let ident = identifier_at(&doc.analysis.identifiers, at)?;
        Some((doc.path.clone(), ident.start))
    }
}
