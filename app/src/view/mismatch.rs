//! The commit-mismatch banner: a link records the commit it was made at, and the checkout
//! may have moved since. Chorong opens the current checkout, resolves the target by symbol, and
//! says what differs (`docs/public/url-scheme.md`, "Commit mismatch"). It offers the file as it was
//! (read from the git object store, read-only), another worktree that has that commit, a
//! fresh graph, and the `git switch` command to copy. It never moves the checkout.

use super::*;

/// What git says about the checkout and the link's commit, gathered off the UI thread.
#[derive(Debug, Clone)]
pub(crate) struct LinkFacts {
    /// The repository's top level (where paths in links are relative to).
    top: PathBuf,
    head: Option<String>,
    branch: Option<String>,
    dirty: bool,
    /// The link's `rev` as a full commit id, if this repository has it.
    rev_full: Option<String>,
    /// Other worktrees at the link's commit (or on its branch), best first.
    worktrees: Vec<PathBuf>,
}

/// Index build state (`cho-semantic` cache meta) for "use that commit's cached index".
const CLEAN_STATUS_HASH: &str = "cbf29ce484222325";

impl LinkFacts {
    fn gather(root: &std::path::Path, info: &LinkInfo) -> Self {
        let top = cho_git::toplevel(root);
        let git = cho_git::info(&top);
        let rev_full = info
            .rev
            .as_deref()
            .and_then(|r| cho_git::rev_parse(&top, r));
        let me = top.canonicalize().ok();
        let mut worktrees: Vec<cho_git::Worktree> = cho_git::worktrees(&top)
            .into_iter()
            .filter(|w| w.path.canonicalize().ok() != me)
            .collect();
        let at_rev = |w: &cho_git::Worktree| rev_full.is_some() && w.head == rev_full;
        worktrees.retain(|w| at_rev(w) || (info.branch.is_some() && w.branch == info.branch));
        worktrees.sort_by_key(|w| !at_rev(w));
        LinkFacts {
            top,
            head: git.full_commit,
            branch: git.branch,
            dirty: git.dirty,
            rev_full,
            worktrees: worktrees.into_iter().map(|w| w.path).collect(),
        }
    }

    fn same_commit(&self) -> bool {
        self.rev_full.is_some() && self.rev_full == self.head
    }
}

/// Where the link was made, for the banner: "`3f9c1e2` on `feature/sessions`".
fn made_at(info: &LinkInfo) -> String {
    let rev = info.rev.as_deref().map(cho_git::short).unwrap_or_default();
    match &info.branch {
        Some(b) => format!("{rev} on {b}"),
        None => rev,
    }
}

fn checkout_at(f: &LinkFacts) -> String {
    let head = f
        .head
        .as_deref()
        .map(cho_git::short)
        .unwrap_or("(no commit)".into());
    match &f.branch {
        Some(b) => format!("{head} on {b}"),
        None => format!("{head} (detached)"),
    }
}

impl Chorong {
    /// A link was opened in this window: gather what git says about its commit, in the
    /// background, then show the banner once the target has been resolved too.
    pub(super) fn link_opened(&mut self, info: LinkInfo, cx: &mut Context<Self>) {
        self.ed_mut().link_facts = None;
        self.ed_mut().link_found = None;
        self.ed_mut().banner = None;
        if info.rev.is_none() {
            self.ed_mut().link = Some(info);
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let probe = info.clone();
        self.ed_mut().link = Some(info);
        let pane = self.active_editor;
        cx.spawn(async move |this, cx| {
            let facts = cx
                .background_spawn(async move { LinkFacts::gather(&root, &probe) })
                .await;
            this.update(cx, |this, cx| {
                if !this.editors.contains_key(&pane) {
                    return;
                }
                this.with_pane(pane, |this| {
                    this.ed_mut().link_facts = Some(facts);
                    this.refresh_link_banner();
                });
                this.dirty_pane = Some(pane);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The checkout's HEAD is (or isn't) the link's commit. Asked synchronously (one fast
    /// `git rev-parse`) when a link's symbol is not found: the link's `line` is only used at
    /// the same commit, never at another one.
    pub(super) fn link_at_head(&self) -> bool {
        let (Some(root), Some(rev)) = (
            self.root.as_ref(),
            self.ed().link.as_ref().and_then(|l| l.rev.as_deref()),
        ) else {
            return true;
        };
        let head = cho_git::git(root, &["rev-parse", "HEAD"]);
        head.is_some_and(|h| h.starts_with(rev))
    }

    /// Builds the banner once both the git facts and the symbol lookup are in.
    pub(super) fn refresh_link_banner(&mut self) {
        let (Some(info), Some(f), Some(found)) =
            (&self.ed().link, &self.ed().link_facts, self.ed().link_found)
        else {
            return;
        };
        let viewing_rev = self.ed().doc.as_ref().is_some_and(|d| d.at_rev.is_some());
        if f.same_commit() {
            self.ed_mut().banner = (info.dirty || f.dirty).then(|| Banner {
                text: "Link made before (or with) local edits; showing the current file.".into(),
                detail: None,
                actions: vec![("Dismiss".into(), BannerAction::Dismiss)],
                quiet: true,
            });
            return;
        }
        let rev = info.rev.as_deref().map(cho_git::short).unwrap_or_default();
        let here = f.head.as_deref().map(cho_git::short).unwrap_or_default();
        let mut text = match (&info.symbol, found) {
            (Some(sym), false) => format!(
                "`{sym}` not found at {here}. The link was made at {}.",
                made_at(info)
            ),
            _ => format!(
                "Link made at {}; this checkout is at {}.",
                made_at(info),
                checkout_at(f)
            ),
        };
        if info.pr.is_some() && f.rev_full.is_none() {
            // A pull-request link whose version Chorong does not have: say that, and that the
            // window shows the current head.
            text = links::gone_text(&rev, &here, info.fetched);
        } else if f.rev_full.is_none() {
            text.push_str(&format!(
                " {rev} is not in this repository; fetch it to view the file as it was."
            ));
        }
        let mut actions = Vec::new();
        if f.rev_full.is_some() && info.path.is_some() {
            actions.push(if viewing_rev {
                ("Back to the checkout".into(), BannerAction::BackToCheckout)
            } else {
                (format!("View at {rev}"), BannerAction::ViewAtRev)
            });
        }
        if let Some(w) = f.worktrees.first() {
            let name = w
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            actions.push((
                format!("Open worktree {name}"),
                BannerAction::OpenWorktree(w.clone()),
            ));
        }
        if info.from_graph {
            actions.push(("Rebuild graph".into(), BannerAction::RebuildGraph));
        }
        // A pull-request version Chorong does not have cannot be switched to.
        let gone = info.pr.is_some() && f.rev_full.is_none();
        let switch = info
            .rev
            .as_deref()
            .filter(|_| !gone)
            .and_then(|r| cho_git::switch_command(r, info.branch.as_deref()));
        if let Some(cmd) = &switch {
            actions.push((
                "Copy git switch command".into(),
                BannerAction::Copy(cmd.clone()),
            ));
        }
        actions.push(("Dismiss".into(), BannerAction::Dismiss));
        self.ed_mut().banner = Some(Banner {
            text,
            detail: switch.map(|c| format!("To move the checkout yourself: {c}")),
            actions,
            quiet: false,
        });
    }

    /// "View at <rev>": the linked file as it was, read from the object store. Read-only;
    /// semantic features use an index only if the cache holds one built at exactly that
    /// commit on a clean tree, otherwise the view is syntax-only and says so.
    pub(super) fn view_at_rev(&mut self, cx: &mut Context<Self>) {
        let (Some(info), Some(f)) = (self.ed().link.clone(), self.ed().link_facts.clone()) else {
            return;
        };
        let (Some(rev), Some(path)) = (f.rev_full.clone(), info.path.clone()) else {
            return;
        };
        let short = cho_git::short(&rev);
        let file = f.top.join(&path);
        let display = self
            .root
            .as_deref()
            .and_then(|r| file.strip_prefix(r).ok())
            .map_or(path.clone(), |p| p.display().to_string());
        let theme = self.theme.clone();
        let json = self.features.on(Feature::JsonFolds);
        let top = f.top.clone();
        let generation = self.ed_mut().generation.next();
        let pane = self.active_editor;
        self.remember_caret();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let bytes = cho_git::read_file_at(&top, &rev, &path)?;
                    let doc = Document::from_bytes(file, display, bytes, Some(short), &theme, json)
                        .map_err(|e| e.to_string())?;
                    Ok::<_, String>((doc, rev_index_matches(&top, &rev)))
                })
                .await;
            this.update(cx, |this, cx| {
                if !this.editor_is(pane, generation) {
                    return;
                }
                match loaded {
                    Ok((doc, index_ok)) => {
                        this.with_pane(pane, |this| {
                            this.install(doc);
                            this.ed_mut().rev_index_ok = index_ok;
                            let target = Target {
                                symbol: info.symbol.clone(),
                                line: info.line.map(|l| l.saturating_sub(1)),
                                ..Target::default()
                            };
                            this.apply_target(&target, cx);
                            this.refresh_call_kinds(cx);
                            this.record_history();
                            this.refresh_link_banner();
                        });
                        this.sync_editor_pane(pane, cx);
                        this.refresh_context(cx);
                    }
                    Err(e) => this.notify_text(e, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Leaves the view at another commit for the same file in the checkout.
    pub(super) fn back_to_checkout(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = &self.ed().doc else { return };
        let path = doc.path.clone();
        let target = Target {
            symbol: self.ed().link.as_ref().and_then(|l| l.symbol.clone()),
            ..Target::default()
        };
        self.open(path, target, true, cx);
    }

    /// "Open worktree": the same link, in the worktree that has its commit or branch.
    pub(super) fn open_worktree(&mut self, worktree: PathBuf, cx: &mut Context<Self>) {
        let Some(info) = &self.ed().link else { return };
        let link = cho_core::ChorongLink::Open {
            repo: cho_core::RepoRef::Local {
                path: worktree.display().to_string(),
            },
            pr: info.pr,
            rev: info.rev.clone(),
            base: info.base.clone(),
            side: info.side,
            path: info.path.clone(),
            symbol: info.symbol.clone(),
            line: info.line,
            branch: info.branch.clone(),
            dirty: info.dirty,
        };
        crate::send_link(link.to_url(), cx);
    }
}

/// Whether the cached index for `top` was built at exactly `rev` on a clean tree, so it
/// describes the file as it was at that commit.
pub(super) fn rev_index_matches(top: &std::path::Path, rev: &str) -> bool {
    let Some(cache) = IndexCache::default_location() else {
        return false;
    };
    let Ok(top) = top.canonicalize() else {
        return false;
    };
    let projects: Vec<_> = cache
        .projects(&top)
        .into_iter()
        .filter(|p| p.has_index)
        .collect();
    !projects.is_empty()
        && projects.iter().all(|p| {
            p.meta.git_head.as_deref() == Some(rev)
                && p.meta.git_status_hash.as_deref() == Some(CLEAN_STATUS_HASH)
        })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_clean_status_hash_is_the_hash_of_an_empty_status() {
        assert_eq!(
            format!("{:016x}", cho_semantic::cache::fnv1a(b"")),
            super::CLEAN_STATUS_HASH
        );
    }
}
